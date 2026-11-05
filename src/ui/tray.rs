//! The tray icon as a StatusNotifierItem (SNI). GTK4 has no tray API, and the Omarchy
//! shell (quickshell) hosts `org.kde.StatusNotifierWatcher`, so the item is served over
//! the session bus from the UI thread's glib main loop. `Activate` (the host's click)
//! presents the window; the icon is the project's own themed SVG (`uu-remote-linux`,
//! contrib/desktop), never a NetEase asset.

use nsh_platform_linux as platform;
use platform::gtk4::{gio, glib};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";

const NODE_XML: &str = r#"
<node>
  <interface name="org.kde.StatusNotifierItem">
    <method name="Activate">
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
    </method>
    <method name="SecondaryActivate">
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
    </method>
    <method name="Scroll">
      <arg name="delta" type="i" direction="in"/>
      <arg name="orientation" type="s" direction="in"/>
    </method>
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="AttentionIconName" type="s" access="read"/>
    <property name="OverlayIconName" type="s" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <property name="WindowId" type="i" access="read"/>
  </interface>
</node>"#;

/// Serves the item and registers with the watcher; re-registers when the tray host
/// restarts (a new owner of the watcher name). Runs once, on the UI thread; the
/// registration guard is forgotten — the item lives until the process exits and the
/// bus cleans up.
pub(crate) fn start() {
    use gio::prelude::*;
    use glib::variant::ToVariant;

    let Ok(connection) = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>) else {
        log::info!("tray: no session bus");
        return;
    };
    let title = crate::i18n::s("UU 远程 Linux 主机（非官方）", "UU Remote Linux host (unofficial)");
    let subtitle = crate::i18n::s("点击打开主窗口", "Click to open the main window");
    let tooltip = {
        use glib::variant::ToVariant;
        glib::Variant::tuple_from_iter([
            "uu-remote-linux".to_variant(),
            Vec::<(i32, i32, Vec<u8>)>::new().to_variant(), // no pixmap; the themed name carries it
            title.to_variant(),
            subtitle.to_variant(),
        ])
    };
    let property = move |_: gio::DBusConnection, _: Option<&str>, _: &str, _: &str, name: &str| -> glib::Variant {
        use glib::variant::ToVariant;
        match name {
            "Category" => "ApplicationStatus".to_variant(),
            "Id" => "org.uuremote.Server".to_variant(),
            "Title" => title.to_variant(),
            "Status" => "Active".to_variant(),
            "IconName" => "uu-remote-linux".to_variant(),
            "AttentionIconName" | "OverlayIconName" => "".to_variant(),
            "ToolTip" => tooltip.clone(),
            "ItemIsMenu" => false.to_variant(),
            "Menu" => "/".to_variant(),
            "WindowId" => 0i32.to_variant(),
            _ => "".to_variant(),
        }
    };
    let Some(node) = gio::DBusNodeInfo::for_xml(NODE_XML).ok() else { return };
    let Some(interface) = node.lookup_interface("org.kde.StatusNotifierItem") else { return };
    let built = connection
        .register_object(ITEM_PATH, &interface)
        // Every activation form presents the window (run_on_ui_thread queues us safely
        // even though the DBus dispatch already runs on the UI thread).
        .method_call(|_, _, _, _, _, _, invocation| {
            super::present();
            invocation.return_value(None::<&glib::Variant>);
        })
        .property(property)
        .set_property(|_, _, _, _, _, _| false)
        .build();
    if let Err(error) = built {
        log::warn!("tray: register_object failed: {error}");
        return;
    }
    if let Err(error) = register(&connection) {
        log::info!("tray: watcher registration failed: {error}");
    }
    // The host (quickshell) may restart; a new watcher owner gets our item again.
    let signal_connection = connection.clone();
    connection.signal_subscribe(
        None,
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        None,
        None,
        gio::DBusSignalFlags::NONE,
        move |_, _, _, _, _, params| {
            let Some((name, _old, new)) = params.get::<(String, String, String)>() else { return };
            if name == WATCHER && !new.is_empty() {
                if let Err(error) = register(&signal_connection) {
                    log::info!("tray: watcher registration failed: {error}");
                }
            }
        },
    );
}

/// `RegisterStatusNotifierItem(<our unique bus name>)`: the item lives at a fixed path
/// on our connection. DBus method arguments are always a tuple around the signature.
fn register(connection: &gio::DBusConnection) -> Result<(), glib::Error> {
    use gio::prelude::*;
    use glib::variant::ToVariant;
    let unique = connection.unique_name().unwrap_or_default();
    let arguments = glib::Variant::tuple_from_iter([unique.to_variant()]);
    connection.call_sync(
        Some(WATCHER),
        "/StatusNotifierWatcher",
        WATCHER,
        "RegisterStatusNotifierItem",
        Some(&arguments),
        None,
        gio::DBusCallFlags::NONE,
        3000,
        None::<&gio::Cancellable>,
    )?;
    Ok(())
}
