/// Flatpak document grants and host services need portal-specific handling.
pub(in crate::platform) fn is_flatpak() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/.flatpak-info").is_file() || std::env::var_os("FLATPAK_ID").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
pub(in crate::platform) fn trash_file(path: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::os::fd::AsFd;

    // The portal requires write access. Keep the file alive through the call so
    // the borrowed descriptor remains valid, without modifying its contents.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .context("Could not open file for the Trash portal")?;
    let connection = zbus::blocking::Connection::session()
        .context("Could not connect to the desktop portal session bus")?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Trash",
    )
    .context("Could not access the Trash portal")?;
    let result: u32 = proxy
        .call("TrashFile", &(zbus::zvariant::Fd::from(file.as_fd()),))
        .context("Trash portal call failed")?;
    anyhow::ensure!(result == 1, "Trash portal failed (result {result})");
    Ok(())
}

/// Asks the host file manager to show `path` selected. The portal itself falls
/// back to opening the folder when the host cannot select the file.
#[cfg(target_os = "linux")]
pub(in crate::platform) fn open_directory(path: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::os::fd::AsFd;

    // Keep the file alive through the call so the borrowed descriptor remains valid.
    let file = std::fs::File::open(path).context("Could not open file for the OpenURI portal")?;
    let connection = zbus::blocking::Connection::session()
        .context("Could not connect to the desktop portal session bus")?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.OpenURI",
    )
    .context("Could not access the OpenURI portal")?;
    let options = std::collections::HashMap::<&str, zbus::zvariant::Value>::new();
    let _request: zbus::zvariant::OwnedObjectPath = proxy
        .call(
            "OpenDirectory",
            &("", zbus::zvariant::Fd::from(file.as_fd()), options),
        )
        .context("OpenURI portal call failed")?;
    Ok(())
}
