//! Keeps NVIDIA G-SYNC off for the CalibRaw window on Windows.
//!
//! CalibRaw repaints only when input arrives. When the driver applies G-SYNC to
//! windowed programs, the monitor's refresh rate then follows the mouse and falls
//! back when it stops, which panels show as flicker. A program cannot opt out of
//! variable refresh when it presents, so at startup this sets G-SYNC to
//! fullscreen-only in CalibRaw's profile of the NVIDIA driver settings, as Godot
//! does for its editor. CalibRaw has no fullscreen mode, so that turns it off.
//!
//! A value the profile already holds, chosen in NVIDIA's tools or written by an
//! earlier start, is left alone, so the driver database is written at most once.

use std::ffi::{c_void, OsStr};
use std::fmt;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null_mut;
use std::time::Instant;
use windows_sys::core::{s, w, PCWSTR};
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

#[cfg(target_pointer_width = "64")]
const NVAPI_LIBRARY: PCWSTR = w!("nvapi64.dll");
#[cfg(not(target_pointer_width = "64"))]
const NVAPI_LIBRARY: PCWSTR = w!("nvapi.dll");

/// Name of the profile created when no profile lists CalibRaw's executable.
const PROFILE_NAME: &str = "CalibRaw";

/// `VRR_MODE_ID` and `VRR_MODE_FULLSCREEN_ONLY` from NVIDIA's
/// NvApiDriverSettings.h.
const VRR_MODE_ID: u32 = 0x1194_F158;
const VRR_MODE_FULLSCREEN_ONLY: u32 = 1;

// Function IDs for `nvapi_QueryInterface`, from NVIDIA's nvapi_interface.h.
const NVAPI_INITIALIZE: u32 = 0x0150_E828;
const NVAPI_UNLOAD: u32 = 0xD22B_DD7E;
const NVAPI_DRS_CREATE_SESSION: u32 = 0x0694_D52E;
const NVAPI_DRS_DESTROY_SESSION: u32 = 0xDAD9_CFF8;
const NVAPI_DRS_LOAD_SETTINGS: u32 = 0x375D_BD6B;
const NVAPI_DRS_SAVE_SETTINGS: u32 = 0xFCBC_7E14;
const NVAPI_DRS_FIND_APPLICATION_BY_NAME: u32 = 0xEEE5_66B2;
const NVAPI_DRS_FIND_PROFILE_BY_NAME: u32 = 0x7E4A_9A0B;
const NVAPI_DRS_CREATE_PROFILE: u32 = 0xCC17_6068;
const NVAPI_DRS_CREATE_APPLICATION: u32 = 0x4347_A9DE;
const NVAPI_DRS_GET_SETTING: u32 = 0x73BF_8338;
const NVAPI_DRS_SET_SETTING: u32 = 0x577D_D202;

// `NvAPI_Status` values, from NVIDIA's nvapi_lite_common.h.
const NVAPI_OK: i32 = 0;
const NVAPI_LIBRARY_NOT_FOUND: i32 = -2;
const NVAPI_NVIDIA_DEVICE_NOT_FOUND: i32 = -6;
const NVAPI_SETTING_NOT_FOUND: i32 = -160;
const NVAPI_PROFILE_NOT_FOUND: i32 = -163;

const NVDRS_DWORD_TYPE: u32 = 0;
const NVDRS_CURRENT_PROFILE_LOCATION: u32 = 0;

const UNICODE_STRING_MAX: usize = 2048;
/// `NvAPI_UnicodeString`: NUL-terminated UTF-16.
type UnicodeString = [u16; UNICODE_STRING_MAX];

type SessionHandle = *mut c_void;
type ProfileHandle = *mut c_void;

type QueryInterfaceFn = unsafe extern "C" fn(u32) -> *mut c_void;
type StatusFn = unsafe extern "C" fn() -> i32;
type CreateSessionFn = unsafe extern "C" fn(*mut SessionHandle) -> i32;
type SessionFn = unsafe extern "C" fn(SessionHandle) -> i32;
type FindApplicationByNameFn =
    unsafe extern "C" fn(SessionHandle, *mut u16, *mut ProfileHandle, *mut DrsApplication) -> i32;
type FindProfileByNameFn = unsafe extern "C" fn(SessionHandle, *mut u16, *mut ProfileHandle) -> i32;
type CreateProfileFn =
    unsafe extern "C" fn(SessionHandle, *mut DrsProfile, *mut ProfileHandle) -> i32;
type CreateApplicationFn =
    unsafe extern "C" fn(SessionHandle, ProfileHandle, *mut DrsApplication) -> i32;
type GetSettingFn = unsafe extern "C" fn(SessionHandle, ProfileHandle, u32, *mut DrsSetting) -> i32;
type SetSettingFn = unsafe extern "C" fn(SessionHandle, ProfileHandle, *mut DrsSetting) -> i32;

/// `NVDRS_SETTING_V1` (nvapi.h packs it to 4 bytes).
#[repr(C)]
struct DrsSetting {
    version: u32,
    setting_name: UnicodeString,
    setting_id: u32,
    setting_type: u32,
    setting_location: u32,
    is_current_predefined: u32,
    is_predefined_valid: u32,
    /// Union of a DWORD, a QWORD, a string and a 4100-byte binary value; a
    /// DWORD setting uses the first word.
    predefined_value: [u32; 1025],
    current_value: [u32; 1025],
}

/// `NVDRS_APPLICATION_V4`.
#[repr(C)]
struct DrsApplication {
    version: u32,
    is_predefined: u32,
    app_name: UnicodeString,
    user_friendly_name: UnicodeString,
    launcher: UnicodeString,
    file_in_folder: UnicodeString,
    /// Bit fields `isMetro`, `isCommandLine` and reserved bits.
    flags: u32,
    command_line: UnicodeString,
}

/// `NVDRS_PROFILE_V1`.
#[repr(C)]
struct DrsProfile {
    version: u32,
    profile_name: UnicodeString,
    gpu_support: u32,
    is_predefined: u32,
    num_of_apps: u32,
    num_of_settings: u32,
}

// The sizes nvapi.h gives these structures, which their versions encode.
const _: () = assert!(size_of::<DrsSetting>() == 12_320);
const _: () = assert!(size_of::<DrsApplication>() == 20_492);
const _: () = assert!(size_of::<DrsProfile>() == 4_116);
const _: () = assert!(std::mem::offset_of!(DrsSetting, setting_location) == 4_108);
const _: () = assert!(std::mem::offset_of!(DrsSetting, current_value) == 8_220);
const _: () = assert!(std::mem::offset_of!(DrsApplication, command_line) == 16_396);

/// `MAKE_NVAPI_VERSION`: the structure size with the version in the high word.
const fn struct_version<T>(version: u32) -> u32 {
    size_of::<T>() as u32 | (version << 16)
}

/// Heap-allocated, zeroed driver structure; the largest is 20 KiB.
fn zeroed<T: DriverStruct>() -> Box<T> {
    // SAFETY: `DriverStruct` types hold only integers and integer arrays, for
    // which all-zero bytes are a valid value.
    unsafe { Box::<T>::new_zeroed().assume_init() }
}

/// A `#[repr(C)]` NVAPI structure of integers only.
trait DriverStruct {}
impl DriverStruct for DrsSetting {}
impl DriverStruct for DrsApplication {}
impl DriverStruct for DrsProfile {}
impl DriverStruct for UnicodeString {}

/// Sets G-SYNC to fullscreen-only for CalibRaw where an NVIDIA driver is
/// installed and records what happened in the diagnostics. Runs before the
/// window opens because the driver reads the profile when the process creates
/// its device.
pub(super) fn keep_gsync_off_for_window() {
    let started = Instant::now();
    match apply() {
        Ok(Outcome::NoNvidiaDriver) => {}
        Ok(Outcome::AlreadySet(value)) => calibraw_core::diagnostics::record(format!(
            "NVIDIA driver profile keeps G-SYNC mode {value} for CalibRaw"
        )),
        Ok(Outcome::Written) => calibraw_core::diagnostics::record(format!(
            "NVIDIA driver profile: set G-SYNC to fullscreen-only for CalibRaw in {:.3}s",
            started.elapsed().as_secs_f64()
        )),
        // No dialog: CalibRaw works without the setting, and an error at
        // every start would not help a user who cannot change the driver.
        Err(error) => calibraw_core::diagnostics::record(format!(
            "NVIDIA driver profile not changed, windowed G-SYNC may flicker: {error}"
        )),
    }
}

enum Outcome {
    NoNvidiaDriver,
    /// The profile already holds a G-SYNC mode, this value.
    AlreadySet(u32),
    Written,
}

#[derive(Debug)]
enum Error {
    MissingFunction,
    ExecutablePath(std::io::Error),
    NameTooLong,
    Status { call: &'static str, status: i32 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFunction => f.write_str("the driver's NVAPI lacks a settings function"),
            Self::ExecutablePath(error) => write!(f, "executable path unavailable: {error}"),
            Self::NameTooLong => f.write_str("executable path too long for NVAPI"),
            Self::Status { call, status } => write!(f, "{call} returned status {status}"),
        }
    }
}

fn check(call: &'static str, status: i32) -> Result<(), Error> {
    if status == NVAPI_OK {
        Ok(())
    } else {
        Err(Error::Status { call, status })
    }
}

fn apply() -> Result<Outcome, Error> {
    let Some(library) = Library::load() else {
        return Ok(Outcome::NoNvidiaDriver);
    };
    let api = Api::load(library).ok_or(Error::MissingFunction)?;
    // SAFETY: NvAPI_Initialize takes no arguments.
    match unsafe { (api.initialize)() } {
        NVAPI_OK => {}
        // An NVIDIA driver without an NVIDIA GPU driving a display.
        NVAPI_LIBRARY_NOT_FOUND | NVAPI_NVIDIA_DEVICE_NOT_FOUND => {
            return Ok(Outcome::NoNvidiaDriver)
        }
        status => {
            return Err(Error::Status {
                call: "NvAPI_Initialize",
                status,
            })
        }
    }
    let _initialized = Initialized(&api);
    let session = Session::open(&api)?;
    let executable = std::env::current_exe().map_err(Error::ExecutablePath)?;
    let profile = session.profile_for(&executable)?;
    if let Some(value) = session.own_dword(profile, VRR_MODE_ID)? {
        return Ok(Outcome::AlreadySet(value));
    }
    session.set_dword(profile, VRR_MODE_ID, VRR_MODE_FULLSCREEN_ONLY)?;
    session.save()?;
    Ok(Outcome::Written)
}

/// NVIDIA's NVAPI library, freed on drop.
struct Library(HMODULE);

impl Library {
    fn load() -> Option<Self> {
        // SAFETY: the name is a NUL-terminated literal. Only NVIDIA's driver
        // installs nvapi64.dll, in the system directory.
        let module = unsafe { LoadLibraryW(NVAPI_LIBRARY) };
        (!module.is_null()).then_some(Self(module))
    }

    fn query_interface(&self) -> Option<QueryInterfaceFn> {
        // SAFETY: the module is loaded and the name is a NUL-terminated literal.
        let function = unsafe { GetProcAddress(self.0, s!("nvapi_QueryInterface")) }?;
        // SAFETY: NVAPI exports `void *__cdecl nvapi_QueryInterface(unsigned int)`.
        Some(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryInterfaceFn>(function)
        })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: the module was loaded by `load` and no function pointer from
        // it outlives `Api`, which owns this library.
        unsafe { FreeLibrary(self.0) };
    }
}

/// The NVAPI functions used here, valid while `library` stays loaded.
struct Api {
    initialize: StatusFn,
    unload: StatusFn,
    create_session: CreateSessionFn,
    destroy_session: SessionFn,
    load_settings: SessionFn,
    save_settings: SessionFn,
    find_application_by_name: FindApplicationByNameFn,
    find_profile_by_name: FindProfileByNameFn,
    create_profile: CreateProfileFn,
    create_application: CreateApplicationFn,
    get_setting: GetSettingFn,
    set_setting: SetSettingFn,
    /// Dropped last, after the function pointers above.
    _library: Library,
}

impl Api {
    fn load(library: Library) -> Option<Self> {
        let query = library.query_interface()?;
        macro_rules! function {
            ($id:expr, $type:ty) => {{
                // SAFETY: `nvapi_QueryInterface` takes any ID and returns null
                // for an unknown one.
                let function = unsafe { query($id) };
                if function.is_null() {
                    return None;
                }
                // SAFETY: nvapi_interface.h maps the ID to the function whose
                // C signature `$type` mirrors.
                unsafe { std::mem::transmute::<*mut c_void, $type>(function) }
            }};
        }
        Some(Self {
            initialize: function!(NVAPI_INITIALIZE, StatusFn),
            unload: function!(NVAPI_UNLOAD, StatusFn),
            create_session: function!(NVAPI_DRS_CREATE_SESSION, CreateSessionFn),
            destroy_session: function!(NVAPI_DRS_DESTROY_SESSION, SessionFn),
            load_settings: function!(NVAPI_DRS_LOAD_SETTINGS, SessionFn),
            save_settings: function!(NVAPI_DRS_SAVE_SETTINGS, SessionFn),
            find_application_by_name: function!(
                NVAPI_DRS_FIND_APPLICATION_BY_NAME,
                FindApplicationByNameFn
            ),
            find_profile_by_name: function!(NVAPI_DRS_FIND_PROFILE_BY_NAME, FindProfileByNameFn),
            create_profile: function!(NVAPI_DRS_CREATE_PROFILE, CreateProfileFn),
            create_application: function!(NVAPI_DRS_CREATE_APPLICATION, CreateApplicationFn),
            get_setting: function!(NVAPI_DRS_GET_SETTING, GetSettingFn),
            set_setting: function!(NVAPI_DRS_SET_SETTING, SetSettingFn),
            _library: library,
        })
    }
}

/// A successful `NvAPI_Initialize`, undone on drop.
struct Initialized<'a>(&'a Api);

impl Drop for Initialized<'_> {
    fn drop(&mut self) {
        // SAFETY: NvAPI_Initialize succeeded; sessions are destroyed first
        // because they are declared after this guard.
        unsafe { (self.0.unload)() };
    }
}

/// A driver settings session with the settings database loaded, destroyed on
/// drop.
struct Session<'a> {
    api: &'a Api,
    handle: SessionHandle,
}

impl<'a> Session<'a> {
    fn open(api: &'a Api) -> Result<Self, Error> {
        let mut handle = null_mut();
        // SAFETY: `handle` is a valid out pointer.
        check("NvAPI_DRS_CreateSession", unsafe {
            (api.create_session)(&mut handle)
        })?;
        let session = Self { api, handle };
        // SAFETY: the session handle is open.
        check("NvAPI_DRS_LoadSettings", unsafe {
            (api.load_settings)(handle)
        })?;
        Ok(session)
    }

    /// The profile that lists `executable`, or the CalibRaw profile with the
    /// executable's file name added.
    fn profile_for(&self, executable: &Path) -> Result<ProfileHandle, Error> {
        let mut path = unicode_string(executable.as_os_str())?;
        let mut application = zeroed::<DrsApplication>();
        application.version = struct_version::<DrsApplication>(4);
        let mut profile = null_mut();
        // SAFETY: the session is open, `path` is NUL-terminated and the out
        // pointers are valid. Any failure means no profile lists CalibRaw:
        // NVIDIA documents a not-found status its header does not define.
        let found = unsafe {
            (self.api.find_application_by_name)(
                self.handle,
                path.as_mut_ptr(),
                &mut profile,
                &mut *application,
            )
        };
        if found == NVAPI_OK {
            return Ok(profile);
        }

        let mut name = unicode_string(OsStr::new(PROFILE_NAME))?;
        // SAFETY: as above.
        match unsafe {
            (self.api.find_profile_by_name)(self.handle, name.as_mut_ptr(), &mut profile)
        } {
            NVAPI_OK => {}
            NVAPI_PROFILE_NOT_FOUND => {
                let mut info = zeroed::<DrsProfile>();
                info.version = struct_version::<DrsProfile>(1);
                info.profile_name = *name;
                // SAFETY: `info` is a versioned profile and `profile` a valid
                // out pointer.
                check("NvAPI_DRS_CreateProfile", unsafe {
                    (self.api.create_profile)(self.handle, &mut *info, &mut profile)
                })?;
            }
            status => {
                return Err(Error::Status {
                    call: "NvAPI_DRS_FindProfileByName",
                    status,
                })
            }
        }

        let file_name = executable
            .file_name()
            .ok_or_else(|| Error::ExecutablePath(std::io::ErrorKind::InvalidInput.into()))?;
        let mut application = zeroed::<DrsApplication>();
        application.version = struct_version::<DrsApplication>(4);
        application.app_name = *unicode_string(file_name)?;
        // SAFETY: `profile` came from the driver in this session and
        // `application` is versioned with empty optional strings.
        check("NvAPI_DRS_CreateApplication", unsafe {
            (self.api.create_application)(self.handle, profile, &mut *application)
        })?;
        Ok(profile)
    }

    /// The DWORD `profile` itself holds for `id`, ignoring values inherited
    /// from the global profile or driver defaults.
    fn own_dword(&self, profile: ProfileHandle, id: u32) -> Result<Option<u32>, Error> {
        let mut setting = zeroed::<DrsSetting>();
        setting.version = struct_version::<DrsSetting>(1);
        // SAFETY: `profile` came from the driver in this session and `setting`
        // is versioned.
        match unsafe { (self.api.get_setting)(self.handle, profile, id, &mut *setting) } {
            NVAPI_OK => Ok((setting.setting_location == NVDRS_CURRENT_PROFILE_LOCATION)
                .then_some(setting.current_value[0])),
            NVAPI_SETTING_NOT_FOUND => Ok(None),
            status => Err(Error::Status {
                call: "NvAPI_DRS_GetSetting",
                status,
            }),
        }
    }

    fn set_dword(&self, profile: ProfileHandle, id: u32, value: u32) -> Result<(), Error> {
        let mut setting = zeroed::<DrsSetting>();
        setting.version = struct_version::<DrsSetting>(1);
        setting.setting_id = id;
        setting.setting_type = NVDRS_DWORD_TYPE;
        setting.current_value[0] = value;
        // SAFETY: as in `own_dword`.
        check("NvAPI_DRS_SetSetting", unsafe {
            (self.api.set_setting)(self.handle, profile, &mut *setting)
        })
    }

    /// Writes the session's changes to the driver's settings database.
    fn save(&self) -> Result<(), Error> {
        // SAFETY: the session handle is open.
        check("NvAPI_DRS_SaveSettings", unsafe {
            (self.api.save_settings)(self.handle)
        })
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle was created by NvAPI_DRS_CreateSession and is
        // destroyed only here.
        unsafe { (self.api.destroy_session)(self.handle) };
    }
}

/// `text` as an `NvAPI_UnicodeString`.
fn unicode_string(text: &OsStr) -> Result<Box<UnicodeString>, Error> {
    let mut string = zeroed::<UnicodeString>();
    // The last unit stays the terminating NUL.
    let mut slots = string[..UNICODE_STRING_MAX - 1].iter_mut();
    for unit in text.encode_wide() {
        *slots.next().ok_or(Error::NameTooLong)? = unit;
    }
    Ok(string)
}
