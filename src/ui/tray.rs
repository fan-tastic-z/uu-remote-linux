//! The tray icon as a StatusNotifierItem (SNI). GTK4 has no tray API, and the Omarchy
//! shell (quickshell) hosts `org.kde.StatusNotifierWatcher`, so the item is served over
//! the session bus from the UI thread's glib main loop. `Activate` (the host's click)
//! presents the window; a `com.canonical.dbusmenu` at `/MenuBar` gives the shell a
//! right-click menu (打开主窗口 / 退出) — `Quit` rides the same command queue the window
//! uses, so the host stops its systemd unit instead of being restarted by it. The icon
//! is the project's own themed SVG (`uu-remote-linux`, contrib/desktop), never a NetEase
//! asset.

use nsh_platform_linux as platform;
use platform::gtk4::{gio, glib};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";

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

const MENU_XML: &str = r#"
<node>
  <interface name="com.canonical.dbusmenu">
    <method name="GetLayout">
      <arg name="parentId" type="i" direction="in"/>
      <arg name="recursionDepth" type="i" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="revision" type="u" direction="out"/>
      <arg name="layout" type="(ia{sv}av)" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="properties" type="a(ia{sv})" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg name="id" type="i" direction="in"/>
      <arg name="name" type="s" direction="in"/>
      <arg name="value" type="v" direction="out"/>
    </method>
    <method name="Event">
      <arg name="id" type="i" direction="in"/>
      <arg name="eventId" type="s" direction="in"/>
      <arg name="data" type="v" direction="in"/>
      <arg name="timestamp" type="u" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg name="events" type="a(isvu)" direction="in"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>
    <method name="AboutToShow">
      <arg name="id" type="i" direction="in"/>
      <arg name="needUpdate" type="b" direction="out"/>
    </method>
    <method name="AboutToShowGroup">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="updatesNeeded" type="ai" direction="out"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>
    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
  </interface>
</node>"#;

/// Menu entry ids (root is 0, as the protocol has it).
const MENU_SHOW: i32 = 1;
const MENU_SEPARATOR: i32 = 2;
const MENU_QUIT: i32 = 3;

fn menu_item_type(id: i32) -> &'static str {
    if id == MENU_SEPARATOR { "separator" } else { "standard" }
}

fn menu_item_label(id: i32) -> &'static str {
    if id == MENU_QUIT {
        crate::i18n::s("退出", "Quit")
    } else {
        crate::i18n::s("打开主窗口", "Open main window")
    }
}

/// One menu entry as the protocol's `a{sv}` property dict.
fn menu_item_dict(id: i32) -> glib::Variant {
    use glib::variant::ToVariant;
    let dict = glib::VariantDict::new(None);
    dict.insert("type", menu_item_type(id));
    if menu_item_type(id) == "standard" {
        dict.insert("label", menu_item_label(id));
        dict.insert("enabled", true);
    }
    dict.to_variant()
}

/// The whole menu as the protocol's nested `(ia{sv}av)` layout, revision 1: children are
/// variant-wrapped items of the same fixed type (the protocol's way around D-Bus having no
/// recursive types), so every level — including empty leaves — is one shape. Static tree;
/// hosts just re-read it after `AboutToShow`.
fn menu_layout() -> glib::Variant {
    use glib::variant::ToVariant;
    let item = |id: i32| {
        // NB: `array_from_iter_with_type` takes the ELEMENT type; it wraps it into the array.
        glib::Variant::tuple_from_iter([
            id.to_variant(),
            menu_item_dict(id),
            glib::Variant::array_from_iter_with_type(
                glib::VariantTy::VARIANT, std::iter::empty::<glib::Variant>()), // no children: av
        ])
    };
    let children = [MENU_SHOW, MENU_SEPARATOR, MENU_QUIT].iter().map(|&id| glib::Variant::from_variant(&item(id)));
    let root = {
        let dict = glib::VariantDict::new(None);
        dict.insert("children-display", "submenu");
        glib::Variant::tuple_from_iter([
            0i32.to_variant(),
            dict.to_variant(),
            glib::Variant::array_from_iter_with_type(glib::VariantTy::VARIANT, children),
        ])
    };
    root
}

/// A tray menu click: present the window or quit the host.
fn menu_event(id: i32) {
    match id {
        MENU_SHOW => super::present(),
        MENU_QUIT => super::state::push(super::state::Command::Quit),
        _ => {}
    }
}

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
            "Menu" => MENU_PATH.to_variant(),
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
    register_menu(&connection);
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

/// Serves the right-click menu (`com.canonical.dbusmenu`) at [`MENU_PATH`]. The tree is
/// static; clicks arrive as `Event`/`EventGroup` with `clicked`, and every unknown or
/// optional call gets the protocol's boring answer (empty, unchanged, all-shown).
fn register_menu(connection: &gio::DBusConnection) {
    use gio::prelude::*;
    use glib::variant::ToVariant;

    let Some(node) = gio::DBusNodeInfo::for_xml(MENU_XML).ok() else { return };
    let Some(interface) = node.lookup_interface("com.canonical.dbusmenu") else { return };
    let built = connection
        .register_object(MENU_PATH, &interface)
        .method_call(|_, _, _, _, method, params, invocation| {
            let method = method.rsplit('.').next().unwrap_or(method);
            match method {
                "GetLayout" => {
                    invocation.return_value(Some(&glib::Variant::tuple_from_iter([
                        1u32.to_variant(),
                        menu_layout(),
                    ])));
                }
                "GetGroupProperties" => {
                    let ids = params.get::<(Vec<i32>, Vec<String>)>()
                        .map(|(ids, _)| ids).unwrap_or_default();
                    let entries = ids.iter()
                        .filter(|id| **id >= MENU_SHOW && **id <= MENU_QUIT)
                        .map(|id| glib::Variant::tuple_from_iter([
                            id.to_variant(),
                            menu_item_dict(*id),
                        ]))
                        .collect::<Vec<_>>();
                    invocation.return_value(Some(&glib::Variant::tuple_from_iter([
                        // element "(ia{sv})"; the call wraps it into "a(ia{sv})"
                        glib::Variant::array_from_iter_with_type(
                            glib::VariantTy::new("(ia{sv})").expect("valid type"), entries),
                    ])));
                }
                "GetProperty" => {
                    let (id, name) = params.get::<(i32, String)>().unwrap_or((0, String::new()));
                    let value = match name.as_str() {
                        "type" => menu_item_type(id).to_variant(),
                        "label" => menu_item_label(id).to_variant(),
                        "enabled" => (menu_item_type(id) != "separator").to_variant(),
                        _ => "".to_variant(),
                    };
                    // The declared out arg is `v`: the value rides variant-wrapped.
                    invocation.return_value(Some(&glib::Variant::tuple_from_iter([
                        glib::Variant::from_variant(&value),
                    ])));
                }
                "Event" => {
                    if let Some((id, event, _, _)) = params.get::<(i32, String, glib::Variant, u32)>() {
                        if event == "clicked" { menu_event(id); }
                    }
                    invocation.return_value(None::<&glib::Variant>);
                }
                "EventGroup" => {
                    if let Some(events) = params.get::<Vec<(i32, String, glib::Variant, u32)>>() {
                        for (id, event, _, _) in events {
                            if event == "clicked" { menu_event(id); }
                        }
                    }
                    invocation.return_value(Some(&glib::Variant::tuple_from_iter([
                        Vec::<i32>::new().to_variant(),
                    ])));
                }
                "AboutToShow" => {
                    invocation.return_value(Some(&glib::Variant::tuple_from_iter([true.to_variant()])));
                }
                "AboutToShowGroup" => {
                    invocation.return_value(Some(&glib::Variant::tuple_from_iter([
                        Vec::<i32>::new().to_variant(),
                        Vec::<i32>::new().to_variant(),
                    ])));
                }
                _ => invocation.return_error(gio::DBusError::UnknownMethod, "unknown dbusmenu method"),
            }
        })
        .property(|_, _, _, _, name| -> glib::Variant {
            use glib::variant::ToVariant;
            match name {
                "Version" => 4u32.to_variant(),
                "TextDirection" => "ltr".to_variant(),
                "Status" => "normal".to_variant(),
                _ => Vec::<String>::new().to_variant(), // IconThemePath: the themed name carries it
            }
        })
        .set_property(|_, _, _, _, _, _| false)
        .build();
    if let Err(error) = built {
        log::warn!("tray: menu register_object failed: {error}");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape hosts demarshal (libdbusmenu builds the same: children as `av` of
    /// variant-wrapped `(ia{sv}av)` items — D-Bus has no recursive types).
    #[test]
    fn menu_layout_wire_shape() {
        use glib::variant::ToVariant;
        let layout = menu_layout();
        assert_eq!(layout.type_().as_str(), "(ia{sv}av)");
        assert_eq!(layout.child_value(0).get::<i32>(), Some(0));
        let children = layout.child_value(2);
        assert_eq!(children.n_children(), 3);
        let quit = children.child_value(2).as_variant().expect("variant-wrapped");
        assert_eq!(quit.type_().as_str(), "(ia{sv}av)");
        assert_eq!(quit.child_value(0).get::<i32>(), Some(MENU_QUIT));
        assert_eq!(quit.child_value(2).n_children(), 0); // leaf
        // The separator carries only its type.
        let sep = children.child_value(1).as_variant().expect("variant-wrapped");
        let dict = glib::VariantDict::new(Some(&sep.child_value(1)));
        assert_eq!(dict.lookup::<String>("type").ok().flatten().as_deref(), Some("separator"));
    }
}
