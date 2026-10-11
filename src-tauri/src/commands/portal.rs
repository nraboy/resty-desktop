//! File-chooser portal probe, AppImage only. See docs/decisions.md's "AppImage" entry.
//!
//! The app's file dialogs use `tauri-plugin-dialog`'s `xdg-portal` backend with no GTK fallback,
//! so without `xdg-desktop-portal` every Browse button goes silently inert. `.deb`/`.rpm`
//! installs guarantee the portal through `bundle.linux.*.depends`; an AppImage has no `depends`
//! mechanism, so only there does the app check for itself and let the UI show a banner.

#[cfg(target_os = "linux")]
use std::time::Duration;

/// True when the FileChooser portal can't be reached. Always `false` off Linux and outside an
/// AppImage — packaged installs keep the "no startup probe" ruling in docs/decisions.md.
#[tauri::command]
pub async fn file_portal_missing() -> bool {
    #[cfg(target_os = "linux")]
    {
        let in_appimage = crate::gpu_compat::is_appimage(
            std::env::var("APPIMAGE").ok().as_deref(),
            std::env::var("APPDIR").ok().as_deref(),
        );
        in_appimage && !portal_reachable().await
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Reads the FileChooser interface's `version` property. Unlike a bare `NameHasOwner` check this
/// D-Bus-activates the portal and confirms a backend really serves FileChooser. Any error —
/// no session bus, no portal, no backend, or a 3s timeout — counts as unreachable.
#[cfg(target_os = "linux")]
async fn portal_reachable() -> bool {
    let call = async {
        let conn = zbus::Connection::session().await?;
        conn.call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.portal.FileChooser", "version"),
        )
        .await?;
        Ok::<(), zbus::Error>(())
    };
    matches!(tokio::time::timeout(Duration::from_secs(3), call).await, Ok(Ok(())))
}
