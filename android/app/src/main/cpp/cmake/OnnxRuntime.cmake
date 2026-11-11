# ONNX Runtime for Android, built from pinned source with its WebGPU execution
# provider (Dawn on Vulkan) compiled in, staged as
# ${CALIBRAW_NATIVE_STAGE_ROOT}/onnxruntime/<ABI>/lib/libonnxruntime.so.
# Microsoft's Android packages have no WebGPU provider, so the GitHub, Google
# Play and F-Droid builds all link this library (ORT_LIB_LOCATION).
#
# The build takes most of an hour, far longer than the other native
# dependencies, and every Gradle variant configures this project in its own
# folder. A staged library whose stamp names this file's content and the NDK is
# therefore reused instead of being built again.
#
# Network access during the build: ONNX Runtime's configure step fetches the
# archives pinned by SHA1 in its cmake/deps.txt, and Dawn's fetches the Git
# commits pinned in its DEPS file. The prebuilt protoc that ONNX Runtime would
# download for cross builds is replaced by one built here from the pinned
# protobuf source. Telemetry (the 1DS SDK) is not built.

set(CALIBRAW_ONNXRUNTIME_VERSION 1.30.0)
# Keep in step with ONNX Runtime 1.30.0's cmake/deps.txt, whose SHA1s these
# archives match: protoc must be the protobuf version ONNX Runtime compiles.
set(CALIBRAW_ONNXRUNTIME_PROTOBUF_VERSION 33.6)
set(CALIBRAW_ONNXRUNTIME_ABSEIL_VERSION 20250814.0)

set(CALIBRAW_ONNXRUNTIME_STAGE "${CALIBRAW_NATIVE_STAGE_ROOT}/onnxruntime/${ANDROID_ABI}")
set(CALIBRAW_ONNXRUNTIME_LIBRARY "${CALIBRAW_ONNXRUNTIME_STAGE}/lib/libonnxruntime.so")
set(CALIBRAW_ONNXRUNTIME_STAMP "${CALIBRAW_ONNXRUNTIME_STAGE}/calibraw-build.stamp")

file(SHA256 "${CMAKE_CURRENT_LIST_FILE}" CALIBRAW_ONNXRUNTIME_RECIPE_HASH)
set(CALIBRAW_ONNXRUNTIME_STAMP_TEXT
    "onnxruntime ${CALIBRAW_ONNXRUNTIME_VERSION}\nrecipe ${CALIBRAW_ONNXRUNTIME_RECIPE_HASH}\nndk ${CMAKE_ANDROID_NDK}\nabi ${ANDROID_ABI}\napi ${CALIBRAW_ANDROID_MIN_SDK}\n")

set(CALIBRAW_ONNXRUNTIME_STAGED_TEXT "")
if(EXISTS "${CALIBRAW_ONNXRUNTIME_STAMP}" AND EXISTS "${CALIBRAW_ONNXRUNTIME_LIBRARY}")
    file(READ "${CALIBRAW_ONNXRUNTIME_STAMP}" CALIBRAW_ONNXRUNTIME_STAGED_TEXT)
endif()

if(CALIBRAW_ONNXRUNTIME_STAGED_TEXT STREQUAL CALIBRAW_ONNXRUNTIME_STAMP_TEXT)
    message(STATUS "ONNX Runtime ${CALIBRAW_ONNXRUNTIME_VERSION}: reusing ${CALIBRAW_ONNXRUNTIME_LIBRARY}")
    add_custom_target(calibraw_onnxruntime)
    return()
endif()

# ONNX Runtime 1.30 needs CMake 3.28, newer than the 3.22.1 AGP runs this
# project with. Use the SDK's CMake package of the version Cargo.toml pins
# (`android_onnxruntime_cmake_version`), installed beside it, unless
# CALIBRAW_ONNXRUNTIME_CMAKE names another.
if(NOT CALIBRAW_ONNXRUNTIME_CMAKE)
    if(NOT CALIBRAW_ONNXRUNTIME_CMAKE_VERSION)
        message(FATAL_ERROR "CALIBRAW_ONNXRUNTIME_CMAKE_VERSION is not set; build through Gradle")
    endif()
    get_filename_component(CALIBRAW_SDK_CMAKE_ROOT "${CMAKE_COMMAND}" DIRECTORY)
    get_filename_component(CALIBRAW_SDK_CMAKE_ROOT "${CALIBRAW_SDK_CMAKE_ROOT}/../.." ABSOLUTE)
    set(CALIBRAW_ONNXRUNTIME_CMAKE
        "${CALIBRAW_SDK_CMAKE_ROOT}/${CALIBRAW_ONNXRUNTIME_CMAKE_VERSION}/bin/cmake")
endif()
if(NOT EXISTS "${CALIBRAW_ONNXRUNTIME_CMAKE}")
    message(FATAL_ERROR
        "ONNX Runtime needs CMake ${CALIBRAW_ONNXRUNTIME_CMAKE_VERSION}, not found at "
        "${CALIBRAW_ONNXRUNTIME_CMAKE}. Install it with: "
        "sdkmanager 'cmake;${CALIBRAW_ONNXRUNTIME_CMAKE_VERSION}'")
endif()
get_filename_component(CALIBRAW_ONNXRUNTIME_CMAKE_BIN "${CALIBRAW_ONNXRUNTIME_CMAKE}" DIRECTORY)
find_program(CALIBRAW_ONNXRUNTIME_NINJA NAMES ninja
    HINTS "${CALIBRAW_ONNXRUNTIME_CMAKE_BIN}" NO_DEFAULT_PATH)
if(NOT CALIBRAW_ONNXRUNTIME_NINJA)
    set(CALIBRAW_ONNXRUNTIME_NINJA "${CMAKE_MAKE_PROGRAM}")
endif()
find_program(CALIBRAW_GIT_EXECUTABLE NAMES git REQUIRED)
find_program(CALIBRAW_PATCH_EXECUTABLE NAMES patch REQUIRED)

# Some translation units need more than 2 GiB to compile; set this (or the
# environment variable of the same name) lower than the core count on machines
# with little memory.
if(NOT CALIBRAW_ONNXRUNTIME_BUILD_JOBS AND DEFINED ENV{CALIBRAW_ONNXRUNTIME_BUILD_JOBS})
    set(CALIBRAW_ONNXRUNTIME_BUILD_JOBS "$ENV{CALIBRAW_ONNXRUNTIME_BUILD_JOBS}")
endif()
if(NOT CALIBRAW_ONNXRUNTIME_BUILD_JOBS)
    set(CALIBRAW_ONNXRUNTIME_BUILD_JOBS "${CALIBRAW_BUILD_JOBS}")
endif()

set(CALIBRAW_ONNXRUNTIME_ROOT "${CALIBRAW_EXTERNAL_ROOT}/onnxruntime")

# -----------------------------------------------------------------------------
# protoc for the build machine
# -----------------------------------------------------------------------------
ExternalProject_Add(calibraw_onnxruntime_abseil_source
    URL https://github.com/abseil/abseil-cpp/archive/refs/tags/${CALIBRAW_ONNXRUNTIME_ABSEIL_VERSION}.zip
    URL_HASH SHA256=b2bdcf6682d8cb53df365bcc5d6c318a22e55821d9978a10fdb61404c026daff
    PREFIX "${CALIBRAW_ONNXRUNTIME_ROOT}/abseil-prefix"
    SOURCE_DIR "${CALIBRAW_ONNXRUNTIME_ROOT}/abseil-src"
    CONFIGURE_COMMAND ""
    BUILD_COMMAND ""
    INSTALL_COMMAND ""
)

# Built with the build machine's own compiler: no toolchain file is passed and
# Gradle sets no CC or CXX. Fetching is forced so that a system Abseil cannot
# replace the pinned source.
set(CALIBRAW_HOST_PROTOC "${CALIBRAW_ONNXRUNTIME_ROOT}/protoc-build/protoc")
ExternalProject_Add(calibraw_onnxruntime_host_protoc
    URL https://github.com/protocolbuffers/protobuf/releases/download/v${CALIBRAW_ONNXRUNTIME_PROTOBUF_VERSION}/protobuf-${CALIBRAW_ONNXRUNTIME_PROTOBUF_VERSION}.tar.gz
    URL_HASH SHA256=16498d7dc7967e9b100632138babd4b86b61592beeccdd556f67539d9c231355
    PREFIX "${CALIBRAW_ONNXRUNTIME_ROOT}/protoc-prefix"
    BINARY_DIR "${CALIBRAW_ONNXRUNTIME_ROOT}/protoc-build"
    DEPENDS calibraw_onnxruntime_abseil_source
    CONFIGURE_COMMAND
        "${CMAKE_COMMAND}" -E env --unset=CMAKE_TOOLCHAIN_FILE --unset=CC --unset=CXX
        "${CALIBRAW_ONNXRUNTIME_CMAKE}"
        -S <SOURCE_DIR>
        -B <BINARY_DIR>
        -G Ninja
        "-DCMAKE_MAKE_PROGRAM=${CALIBRAW_ONNXRUNTIME_NINJA}"
        -DCMAKE_BUILD_TYPE=Release
        -Dprotobuf_BUILD_TESTS=OFF
        -Dprotobuf_BUILD_CONFORMANCE=OFF
        -Dprotobuf_BUILD_EXAMPLES=OFF
        -Dprotobuf_BUILD_SHARED_LIBS=OFF
        -Dprotobuf_INSTALL=OFF
        -Dprotobuf_WITH_ZLIB=OFF
        -Dprotobuf_FORCE_FETCH_DEPENDENCIES=ON
        "-DFETCHCONTENT_SOURCE_DIR_ABSL=${CALIBRAW_ONNXRUNTIME_ROOT}/abseil-src"
        -DFETCHCONTENT_FULLY_DISCONNECTED=ON
        -DABSL_PROPAGATE_CXX_STD=ON
    BUILD_COMMAND
        "${CALIBRAW_ONNXRUNTIME_CMAKE}" --build <BINARY_DIR> --target protoc
        --parallel "${CALIBRAW_ONNXRUNTIME_BUILD_JOBS}"
    INSTALL_COMMAND ""
    BUILD_BYPRODUCTS "${CALIBRAW_HOST_PROTOC}"
)

# -----------------------------------------------------------------------------
# ONNX Runtime with the WebGPU execution provider
# -----------------------------------------------------------------------------
# The options follow `build.py --android --use_webgpu --build_shared_lib
# --android_cpp_shared --no_telemetry --skip_tests` of the same release.
ExternalProject_Add(calibraw_onnxruntime_build
    URL https://github.com/microsoft/onnxruntime/archive/refs/tags/v${CALIBRAW_ONNXRUNTIME_VERSION}.tar.gz
    URL_HASH SHA256=f6681ecbddf53898adf0cc9e8e9e84657485b84d2eca3c8aa353de6d7dd417ef
    PREFIX "${CALIBRAW_ONNXRUNTIME_ROOT}/onnxruntime-prefix"
    BINARY_DIR "${CALIBRAW_ONNXRUNTIME_ROOT}/onnxruntime-build"
    DEPENDS calibraw_onnxruntime_host_protoc
    CONFIGURE_COMMAND
        "${CALIBRAW_ONNXRUNTIME_CMAKE}"
        -S <SOURCE_DIR>/cmake
        -B <BINARY_DIR>
        -G Ninja
        "-DCMAKE_MAKE_PROGRAM=${CALIBRAW_ONNXRUNTIME_NINJA}"
        "-DCMAKE_TOOLCHAIN_FILE=${CMAKE_TOOLCHAIN_FILE}"
        -DANDROID_USE_LEGACY_TOOLCHAIN_FILE=false
        "-DANDROID_NDK=${CMAKE_ANDROID_NDK}"
        "-DANDROID_ABI=${ANDROID_ABI}"
        "-DANDROID_PLATFORM=android-${CALIBRAW_ANDROID_MIN_SDK}"
        "-DANDROID_MIN_SDK=${CALIBRAW_ANDROID_MIN_SDK}"
        -DANDROID_STL=c++_shared
        -DANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON
        -DCMAKE_BUILD_TYPE=Release
        -DCMAKE_COMPILE_WARNING_AS_ERROR=OFF
        "-DPython_EXECUTABLE=${Python3_EXECUTABLE}"
        "-DGIT_EXECUTABLE=${CALIBRAW_GIT_EXECUTABLE}"
        "-DPatch_EXECUTABLE=${CALIBRAW_PATCH_EXECUTABLE}"
        "-DONNX_CUSTOM_PROTOC_EXECUTABLE=${CALIBRAW_HOST_PROTOC}"
        -Donnxruntime_BUILD_SHARED_LIB=ON
        -Donnxruntime_USE_WEBGPU=ON
        -Donnxruntime_USE_TELEMETRY=OFF
        -Donnxruntime_USE_VCPKG=OFF
        -Donnxruntime_USE_MIMALLOC=OFF
        -Donnxruntime_USE_KLEIDIAI=ON
        -Donnxruntime_USE_SVE=ON
        -Donnxruntime_USE_XNNPACK=OFF
        -Donnxruntime_USE_NNAPI_BUILTIN=OFF
        -Donnxruntime_DISABLE_FLOAT8_TYPES=ON
        -Donnxruntime_DISABLE_FLOAT4_TYPES=ON
        -Donnxruntime_BUILD_UNIT_TESTS=OFF
        -Donnxruntime_RUN_ONNX_TESTS=OFF
        -Donnxruntime_BUILD_BENCHMARKS=OFF
        -Donnxruntime_ENABLE_PYTHON=OFF
        -Donnxruntime_BUILD_JAVA=OFF
        -Donnxruntime_BUILD_CSHARP=OFF
        -Donnxruntime_BUILD_NODEJS=OFF
        -Donnxruntime_BUILD_OBJC=OFF
        -Donnxruntime_ENABLE_MICROSOFT_INTERNAL=OFF
        -Donnxruntime_ENABLE_LTO=OFF
        "-DCMAKE_SHARED_LINKER_FLAGS=-Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384"
    BUILD_COMMAND
        "${CALIBRAW_ONNXRUNTIME_CMAKE}" --build <BINARY_DIR> --target onnxruntime
        --parallel "${CALIBRAW_ONNXRUNTIME_BUILD_JOBS}"
    INSTALL_COMMAND
        "${CMAKE_COMMAND}" -E make_directory "${CALIBRAW_ONNXRUNTIME_STAGE}/lib"
        COMMAND "${CMAKE_COMMAND}" -E copy
        <BINARY_DIR>/libonnxruntime.so "${CALIBRAW_ONNXRUNTIME_LIBRARY}"
        COMMAND "${CMAKE_COMMAND}" -E copy
        <SOURCE_DIR>/LICENSE "${CALIBRAW_ONNXRUNTIME_STAGE}/LICENSE"
        COMMAND "${CMAKE_COMMAND}" -E copy
        <SOURCE_DIR>/ThirdPartyNotices.txt "${CALIBRAW_ONNXRUNTIME_STAGE}/ThirdPartyNotices.txt"
    BUILD_BYPRODUCTS "${CALIBRAW_ONNXRUNTIME_LIBRARY}"
)

# Written last, so an interrupted build is not mistaken for a finished one.
set(CALIBRAW_ONNXRUNTIME_STAMP_SOURCE "${CALIBRAW_ONNXRUNTIME_ROOT}/calibraw-build.stamp")
file(WRITE "${CALIBRAW_ONNXRUNTIME_STAMP_SOURCE}" "${CALIBRAW_ONNXRUNTIME_STAMP_TEXT}")
add_custom_target(calibraw_onnxruntime
    COMMAND "${CMAKE_COMMAND}" -E copy
            "${CALIBRAW_ONNXRUNTIME_STAMP_SOURCE}" "${CALIBRAW_ONNXRUNTIME_STAMP}"
    DEPENDS calibraw_onnxruntime_build
    VERBATIM
)
