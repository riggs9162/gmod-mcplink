//! Garry's Mod binary module that exposes a loopback message socket to Lua as the global `mcplink` table.

mod link;
mod lua;

use std::ffi::{c_int, CStr};
use std::sync::{Mutex, MutexGuard, PoisonError};

use link::Link;
use lua::{CFunction, State};

/// Module version reported to Lua.
const VERSION: &str = "0.1.00";

static LINK: Mutex<Option<Link>> = Mutex::new(None);

/// Locks the module's single listener slot, recovering it if a previous holder panicked.
fn link_slot() -> MutexGuard<'static, Option<Link>> {
    LINK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lua `mcplink.Listen()`: starts the listener if needed and returns its port and token, or nil and an error message.
unsafe extern "C-unwind" fn listen(state: State) -> c_int {
    let Some(api) = lua::api() else {
        return 0;
    };

    let mut slot = link_slot();
    if slot.is_none() {
        match Link::listen() {
            Ok(link) => *slot = Some(link),
            Err(error) => {
                (api.lua_pushnil)(state);
                lua::push_bytes(api, state, error.to_string().as_bytes());

                return 2;
            }
        }
    }

    let Some(link) = slot.as_ref() else {
        return 0;
    };

    (api.lua_pushnumber)(state, f64::from(link.port()));
    lua::push_bytes(api, state, link.token().as_bytes());

    2
}

/// Lua `mcplink.Poll()`: returns an array of `{client = number, data = string}` tables, one per request line received since the last call.
unsafe extern "C-unwind" fn poll(state: State) -> c_int {
    let Some(api) = lua::api() else {
        return 0;
    };

    let messages = link_slot().as_ref().map(Link::drain).unwrap_or_default();
    (api.lua_createtable)(state, c_int::try_from(messages.len()).unwrap_or(0), 0);

    for (position, message) in (1..).zip(&messages) {
        (api.lua_createtable)(state, 0, 2);
        (api.lua_pushnumber)(state, message.client as f64);
        (api.lua_setfield)(state, -2, c"client".as_ptr());
        lua::push_bytes(api, state, &message.data);
        (api.lua_setfield)(state, -2, c"data".as_ptr());
        (api.lua_rawseti)(state, -2, position);
    }

    1
}

/// Lua `mcplink.Send(client, data)`: writes one line to a connection and returns whether it was delivered.
unsafe extern "C-unwind" fn send(state: State) -> c_int {
    let Some(api) = lua::api() else {
        return 0;
    };

    let client = (api.lua_tonumber)(state, 1) as u64;
    let delivered = match lua::read_bytes(api, state, 2) {
        Some(data) => link_slot()
            .as_ref()
            .is_some_and(|link| link.send(client, &data)),
        None => false,
    };

    (api.lua_pushboolean)(state, c_int::from(delivered));

    1
}

/// Lua `mcplink.Close()`: stops the listener and drops every connection.
unsafe extern "C-unwind" fn close(_state: State) -> c_int {
    link_slot().take();

    0
}

/// Functions published in the `mcplink` table.
const EXPORTS: &[(&CStr, CFunction)] = &[
    (c"Listen", listen),
    (c"Poll", poll),
    (c"Send", send),
    (c"Close", close),
];

/// Entry point the game calls when Lua runs `require("mcplink")`.
///
/// # Safety
/// `state` must be the valid Lua state the game passes to binary modules.
#[no_mangle]
pub unsafe extern "C-unwind" fn gmod13_open(state: State) -> c_int {
    let Some(api) = lua::api() else {
        return 0;
    };

    (api.lua_createtable)(state, 0, c_int::try_from(EXPORTS.len() + 1).unwrap_or(0));

    for (name, function) in EXPORTS {
        (api.lua_pushcclosure)(state, *function, 0);
        (api.lua_setfield)(state, -2, name.as_ptr());
    }

    lua::push_bytes(api, state, VERSION.as_bytes());
    (api.lua_setfield)(state, -2, c"Version".as_ptr());
    (api.lua_setfield)(state, lua::GLOBALS_INDEX, c"mcplink".as_ptr());

    0
}

/// Entry point the game calls before unloading the module, on shutdown and on every map change.
///
/// # Safety
/// Called by the game on its main thread with the Lua state that loaded the module.
#[no_mangle]
pub unsafe extern "C-unwind" fn gmod13_close(_state: State) -> c_int {
    link_slot().take();

    0
}
