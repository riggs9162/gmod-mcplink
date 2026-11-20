//! Minimal bindings to the Lua C API exported by the game's `lua_shared` library.

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

use libloading::Library;

/// Opaque pointer to a `lua_State`.
pub type State = *mut c_void;

/// Signature of a C function callable from Lua.
pub type CFunction = unsafe extern "C-unwind" fn(State) -> c_int;

/// Pseudo-index of the globals table in Lua 5.1 and LuaJIT.
pub const GLOBALS_INDEX: c_int = -10002;

/// Paths the game loads `lua_shared` from, relative to its working directory, across platforms and branches.
#[cfg(windows)]
const LIBRARY_CANDIDATES: &[&str] = &["lua_shared.dll"];

/// Paths the game loads `lua_shared` from, relative to its working directory, across platforms and branches.
#[cfg(not(windows))]
const LIBRARY_CANDIDATES: &[&str] = &[
    "lua_shared.so",
    "lua_shared_srv.so",
    "bin/linux64/lua_shared.so",
    "bin/linux32/lua_shared.so",
    "bin/lua_shared.so",
    "bin/lua_shared_srv.so",
    "garrysmod/bin/lua_shared.so",
    "garrysmod/bin/lua_shared_srv.so",
];

macro_rules! lua_api {
    ($($name:ident: fn($($argument:ty),*) $(-> $result:ty)?;)*) => {
        /// Function pointers resolved from the already loaded `lua_shared` library.
        pub struct Api {
            _library: Library,
            $(pub $name: unsafe extern "C-unwind" fn($($argument),*) $(-> $result)?,)*
        }

        impl Api {
            /// Resolves every binding from `library`, failing if any symbol is missing.
            unsafe fn load(library: Library) -> Result<Api, libloading::Error> {
                $(
                    let $name = *library.get::<unsafe extern "C-unwind" fn($($argument),*) $(-> $result)?>(
                        concat!(stringify!($name), "\0").as_bytes(),
                    )?;
                )*

                Ok(Api { _library: library, $($name,)* })
            }
        }
    };
}

lua_api! {
    lua_createtable: fn(State, c_int, c_int);
    lua_pushboolean: fn(State, c_int);
    lua_pushcclosure: fn(State, CFunction, c_int);
    lua_pushlstring: fn(State, *const c_char, usize);
    lua_pushnil: fn(State);
    lua_pushnumber: fn(State, f64);
    lua_rawseti: fn(State, c_int, c_int);
    lua_setfield: fn(State, c_int, *const c_char);
    lua_tolstring: fn(State, c_int, *mut usize) -> *const c_char;
    lua_tonumber: fn(State, c_int) -> f64;
}

static API: OnceLock<Option<Api>> = OnceLock::new();

/// Returns the Lua bindings, resolving them on first use, or `None` when `lua_shared` cannot be found.
pub fn api() -> Option<&'static Api> {
    API.get_or_init(|| {
        LIBRARY_CANDIDATES.iter().find_map(|path| unsafe {
            let library = Library::new(path).ok()?;
            Api::load(library).ok()
        })
    })
    .as_ref()
}

/// Pushes `bytes` onto the Lua stack as a string, which may hold arbitrary binary data.
///
/// # Safety
/// `state` must be a valid Lua state with room for one more stack slot.
pub unsafe fn push_bytes(api: &Api, state: State, bytes: &[u8]) {
    (api.lua_pushlstring)(state, bytes.as_ptr().cast(), bytes.len());
}

/// Copies the string at `index` off the Lua stack, or returns `None` when the value is not a string or number.
///
/// # Safety
/// `state` must be a valid Lua state and `index` an acceptable stack index.
pub unsafe fn read_bytes(api: &Api, state: State, index: c_int) -> Option<Vec<u8>> {
    let mut length = 0usize;
    let pointer = (api.lua_tolstring)(state, index, &mut length);
    if pointer.is_null() {
        return None;
    }

    Some(std::slice::from_raw_parts(pointer.cast::<u8>(), length).to_vec())
}
