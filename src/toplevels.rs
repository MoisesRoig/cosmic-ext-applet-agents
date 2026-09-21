// Window activation through COSMIC's toplevel protocols. Every call opens its own
// short-lived Wayland connection, so there is no listener state to keep in sync.
// Under Flatpak the sandbox gets a security context and the compositor withholds
// these protocols, so both entry points degrade to "nothing found".

use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use cosmic_client_toolkit::cosmic_protocols::toplevel_info::v1::client::{
    zcosmic_toplevel_handle_v1 as cosmic_handle, zcosmic_toplevel_info_v1 as cosmic_info,
};
use cosmic_client_toolkit::cosmic_protocols::toplevel_management::v1::client::zcosmic_toplevel_manager_v1 as manager;
use cosmic_client_toolkit::wayland_client::protocol::{wl_registry, wl_seat};
use cosmic_client_toolkit::wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use cosmic_client_toolkit::wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1 as foreign_handle,
    ext_foreign_toplevel_list_v1 as foreign_list,
};

/// Stable per-window ids as the compositor reports them.
pub fn ids() -> Vec<String> {
    let Some((_conn, _queue, state)) = connect() else {
        return Vec::new();
    };
    state
        .toplevels
        .into_iter()
        .map(|(_, identifier)| identifier)
        .collect()
}

/// Raises and focuses the window with this id. `false` when it is gone, which is
/// also what a compositor without the COSMIC extensions reports.
pub fn activate(identifier: &str) -> bool {
    let Some((_conn, mut queue, mut state)) = connect() else {
        return false;
    };
    let (Some(info), Some(manager), Some(seat)) = (&state.info, &state.manager, &state.seat) else {
        return false;
    };
    let Some((handle, _)) = state.toplevels.iter().find(|(_, id)| id == identifier) else {
        return false;
    };

    let qh = queue.handle();
    let toplevel = info.get_cosmic_toplevel(handle, &qh, ());
    manager.activate(&toplevel, seat);
    queue.roundtrip(&mut state).is_ok()
}

/// A second connection to the compositor. Deliberately not `connect_to_env`: the
/// applet's own connection was handed to it on `WAYLAND_SOCKET` by the panel, and
/// claiming that file descriptor twice would corrupt it.
fn open_socket() -> Option<UnixStream> {
    let display = PathBuf::from(std::env::var_os("WAYLAND_DISPLAY")?);
    let path = if display.is_absolute() {
        display
    } else {
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(display)
    };
    UnixStream::connect(path).ok()
}

/// Binds the globals and drains the initial burst of toplevel events.
fn connect() -> Option<(Connection, EventQueue<State>, State)> {
    let conn = Connection::from_socket(open_socket()?).ok()?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::default();
    // One roundtrip for the registry, a second for the toplevels it lets us bind.
    queue.roundtrip(&mut state).ok()?;
    queue.roundtrip(&mut state).ok()?;
    Some((conn, queue, state))
}

#[derive(Default)]
struct State {
    info: Option<cosmic_info::ZcosmicToplevelInfoV1>,
    manager: Option<manager::ZcosmicToplevelManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    toplevels: Vec<(foreign_handle::ExtForeignToplevelHandleV1, String)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "ext_foreign_toplevel_list_v1" => {
                registry.bind::<foreign_list::ExtForeignToplevelListV1, _, _>(name, 1, qh, ());
            }
            // Version 1 announces toplevels on this object itself; from 2 on they are
            // requested per window, which is the only shape handled below.
            "zcosmic_toplevel_info_v1" if version >= 2 => {
                state.info = Some(registry.bind(name, version.min(3), qh, ()));
            }
            "zcosmic_toplevel_manager_v1" => {
                state.manager = Some(registry.bind(name, version.min(4), qh, ()));
            }
            "wl_seat" if state.seat.is_none() => {
                state.seat = Some(registry.bind(name, version.min(5), qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<foreign_list::ExtForeignToplevelListV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &foreign_list::ExtForeignToplevelListV1,
        _: foreign_list::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    cosmic_client_toolkit::wayland_client::event_created_child!(State, foreign_list::ExtForeignToplevelListV1, [
        foreign_list::EVT_TOPLEVEL_OPCODE => (foreign_handle::ExtForeignToplevelHandleV1, ())
    ]);
}

impl Dispatch<foreign_handle::ExtForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &foreign_handle::ExtForeignToplevelHandleV1,
        event: foreign_handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            foreign_handle::Event::Identifier { identifier } => {
                state.toplevels.push((handle.clone(), identifier));
            }
            foreign_handle::Event::Closed => {
                state.toplevels.retain(|(known, _)| known != handle);
            }
            _ => {}
        }
    }
}

/// The remaining objects are only ever written to, so their events are ignored.
macro_rules! ignore_events {
    ($($proxy:ty),* $(,)?) => {
        $(impl Dispatch<$proxy, ()> for State {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        })*
    };
}

ignore_events!(
    wl_seat::WlSeat,
    cosmic_info::ZcosmicToplevelInfoV1,
    cosmic_handle::ZcosmicToplevelHandleV1,
    manager::ZcosmicToplevelManagerV1,
);
