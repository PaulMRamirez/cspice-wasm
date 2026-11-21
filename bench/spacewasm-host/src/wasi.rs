// WASI 0.1 bindings for the SpaceWasm interpreter, over wasi-common.
//
// Adapted from crates/spacewasm_tools/src/bin/spacewasm/run/wasi_preview1.rs in
// https://github.com/nasa/SpaceWasm at v0.8.0-rc.3 (Copyright 2026 California Institute
// of Technology, Apache License 2.0), which in turn derives portions from
// https://github.com/bytecodealliance/wasmtime and its wasi-common crate. Two changes:
// `fd_renumber` is bound (the upstream list omits it, and wasi-libc's freopen, which
// the libf2c runtime inside CSPICE calls, imports it), and every call is counted so the
// harness can report how many system calls one SPICE call makes.
use spacewasm::{HostFunction, HostFunctionBreak, HostFunctionResult, HostModule, Value, vec};
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::ops::ControlFlow;
use std::rc::Rc;
use wasi_common::I32Exit;
use wasi_common::snapshots::preview_1::wasi_snapshot_preview1;
use wiggle::GuestMemory;

/// Run a wiggle-generated call to completion: with the `sync` context every call
/// finishes in a single poll.
fn block_on<F: Future<Output = wiggle::anyhow::Result<T>>, T>(future: F) -> F::Output {
    wiggle::run_in_dummy_executor(future).and_then(|result| result)
}

fn next_i32(it: &mut std::slice::Iter<'_, Value>) -> i32 {
    match it.next() {
        Some(Value::I32(v)) => *v,
        other => panic!("host binding expected an i32 argument, got {other:?}"),
    }
}

fn next_i64(it: &mut std::slice::Iter<'_, Value>) -> i64 {
    match it.next() {
        Some(Value::I64(v)) => *v,
        other => panic!("host binding expected an i64 argument, got {other:?}"),
    }
}

/// `Ok(errno)` goes back to the guest; an unrecoverable host error traps.
fn finish(name: &str, result: wiggle::anyhow::Result<i32>) -> HostFunctionResult {
    match result {
        Ok(code) => ControlFlow::Continue(Some(Value::I32(code))),
        Err(trap) => {
            eprintln!("wasi_snapshot_preview1::{name} trapped: {trap:?}");
            ControlFlow::Break(HostFunctionBreak::Trap)
        }
    }
}

macro_rules! next_arg {
    (i, $it:expr) => {
        next_i32($it)
    };
    (I, $it:expr) => {
        next_i64($it)
    };
}

macro_rules! wasi_binding {
    ($ctx:ident, $calls:ident, $name:ident, $params:literal, [ $( $ty:ident ),* $(,)? ]) => {{
        let ctx = Rc::clone(&$ctx);
        let calls = Rc::clone(&$calls);
        HostFunction::new(
            stringify!($name),
            $params.into(),
            "i".into(),
            move |state, args| {
                calls.set(calls.get() + 1);
                #[allow(unused_mut, unused_variables)]
                let mut it = args.iter();
                finish(
                    stringify!($name),
                    block_on(wasi_snapshot_preview1::$name(
                        &mut *ctx.borrow_mut(),
                        &mut GuestMemory::Shared(state.memory.as_shared_cells()),
                        $( next_arg!($ty, &mut it) ),*
                    )),
                )
            },
        )
    }};
}

/// What the harness shares with the bindings.
pub struct Wasi {
    pub module: HostModule,
    /// The status the guest passed to `proc_exit`, if it called it.
    pub exit_code: Rc<Cell<Option<i32>>>,
    /// Running count of WASI calls the guest has made.
    pub calls: Rc<Cell<u64>>,
}

pub fn make_wasi_preview1_module(wasi_ctx: wasi_common::WasiCtx) -> Wasi {
    let ctx = Rc::new(RefCell::new(wasi_ctx));
    let exit_code: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
    let calls: Rc<Cell<u64>> = Rc::new(Cell::new(0));

    let proc_exit = {
        let ctx = Rc::clone(&ctx);
        let exit_code = Rc::clone(&exit_code);
        let calls = Rc::clone(&calls);
        HostFunction::new("proc_exit", "i".into(), "".into(), move |state, args| {
            calls.set(calls.get() + 1);
            let mut it = args.iter();
            let status = next_i32(&mut it);
            if let Err(e) = block_on(wasi_snapshot_preview1::proc_exit(
                &mut *ctx.borrow_mut(),
                &mut GuestMemory::Shared(state.memory.as_shared_cells()),
                status,
            )) {
                match e.downcast_ref::<I32Exit>() {
                    Some(exit) => exit_code.set(Some(exit.0)),
                    None => eprintln!("wasi_snapshot_preview1::proc_exit trapped: {e:?}"),
                }
            }
            ControlFlow::Break(HostFunctionBreak::Trap)
        })
    };

    let module = HostModule {
        name: "wasi_snapshot_preview1".into(),
        globals: vec![],
        functions: vec![
            wasi_binding!(ctx, calls, args_get, "ii", [i, i]),
            wasi_binding!(ctx, calls, args_sizes_get, "ii", [i, i]),
            wasi_binding!(ctx, calls, environ_get, "ii", [i, i]),
            wasi_binding!(ctx, calls, environ_sizes_get, "ii", [i, i]),
            wasi_binding!(ctx, calls, clock_res_get, "ii", [i, i]),
            wasi_binding!(ctx, calls, clock_time_get, "iIi", [i, I, i]),
            wasi_binding!(ctx, calls, fd_close, "i", [i]),
            wasi_binding!(ctx, calls, fd_fdstat_get, "ii", [i, i]),
            wasi_binding!(ctx, calls, fd_fdstat_set_flags, "ii", [i, i]),
            wasi_binding!(ctx, calls, fd_filestat_get, "ii", [i, i]),
            wasi_binding!(ctx, calls, fd_pread, "iiiIi", [i, i, i, I, i]),
            wasi_binding!(ctx, calls, fd_prestat_get, "ii", [i, i]),
            wasi_binding!(ctx, calls, fd_prestat_dir_name, "iii", [i, i, i]),
            wasi_binding!(ctx, calls, fd_pwrite, "iiiIi", [i, i, i, I, i]),
            wasi_binding!(ctx, calls, fd_read, "iiii", [i, i, i, i]),
            wasi_binding!(ctx, calls, fd_readdir, "iiiIi", [i, i, i, I, i]),
            wasi_binding!(ctx, calls, fd_renumber, "ii", [i, i]),
            wasi_binding!(ctx, calls, fd_seek, "iIii", [i, I, i, i]),
            wasi_binding!(ctx, calls, fd_sync, "i", [i]),
            wasi_binding!(ctx, calls, fd_tell, "ii", [i, i]),
            wasi_binding!(ctx, calls, fd_write, "iiii", [i, i, i, i]),
            wasi_binding!(ctx, calls, path_create_directory, "iii", [i, i, i]),
            wasi_binding!(ctx, calls, path_filestat_get, "iiiii", [i, i, i, i, i]),
            wasi_binding!(ctx, calls, path_open, "iiiiiIIii", [i, i, i, i, i, I, I, i, i]),
            wasi_binding!(ctx, calls, path_readlink, "iiiiii", [i, i, i, i, i, i]),
            wasi_binding!(ctx, calls, path_remove_directory, "iii", [i, i, i]),
            wasi_binding!(ctx, calls, path_rename, "iiiiii", [i, i, i, i, i, i]),
            wasi_binding!(ctx, calls, path_unlink_file, "iii", [i, i, i]),
            wasi_binding!(ctx, calls, poll_oneoff, "iiii", [i, i, i, i]),
            proc_exit,
            wasi_binding!(ctx, calls, random_get, "ii", [i, i]),
            wasi_binding!(ctx, calls, sched_yield, "", []),
        ],
        memory: spacewasm::Vec::zero(),
        table: spacewasm::Vec::zero(),
    };

    Wasi { module, exit_code, calls }
}
