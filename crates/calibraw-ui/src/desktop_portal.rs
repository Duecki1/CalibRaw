/// Flatpak document grants and host services need portal-specific handling.
pub(crate) fn is_flatpak() -> bool {
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
pub(crate) fn trash_file(path: &std::path::Path) -> anyhow::Result<()> {
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
