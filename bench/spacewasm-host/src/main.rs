//! Host harness: full CSPICE as a guest of the SpaceWasm interpreter.
//!
//! Loads `wasm/cspice.wasi.wasm` (the WASI reactor built by
//! `scripts/build-cspice-wasi.sh`) with the `spacewasm` crate, binds WASI 0.1 through
//! `wasi-common`, mounts the kernel fixtures as a preopened directory, and measures:
//! the compiled IR size, the interpreter heap the load takes, guest linear memory, and
//! for each SPICE call the interpreter instructions executed, the number of
//! fixed-fuel slices that is, the wall time, and the WASI calls made.
//!
//! The scenario runs three times on fresh engines. With one instruction of fuel per
//! slice the slice count is the exact instruction count; with `--slice` (default 1000,
//! the default budget of F Prime's `Svc::WasmSequencer`) it is the number of sequencer
//! cycles; with unlimited fuel the wall time has no slicing overhead.
//!
//! Usage: cspice-spacewasm-host --module <cspice.wasi.wasm> --fixtures <dir>
//!          [--slice N] [--repeat N] [--stack-words N] [--json <file>] [--skip-exact]

mod wasi;

use spacewasm::{
    CodeBuilder, CompilerOptions, ControlFrame, DecoderStack, Engine, ExportDesc, InnerVec,
    Interpreter, InterpreterResult, InterpreterRunner, Module, ModuleRef, OperandType, Ref, Value,
    WasmMemoryAllocator, WasmRef, WasmStream,
};
use std::alloc::Layout;
use std::cell::Cell;
use std::fmt::Write as _;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use wasi_common::sync::{Dir, WasiCtxBuilder, ambient_authority};

/// Default operand and call stack for the interpreter, in 32-bit words (4 MiB), the
/// size `spacewasm run` uses. `--stack-words` lowers it to probe the minimum.
const DEFAULT_STACK_WORDS: usize = 0x100_000;
/// Upper bound on IR code pages (512 bytes each); the load reports how many it used.
const MAX_CODE_PAGES: usize = 0x10_000;
/// The decoder stack lengths `spacewasm validate` defaults to, as a flight embedder
/// would pass to `spacewasm_load_module`.
const DECODER_FRAMES: usize = 64;
const DECODER_DEPTH: usize = 256;

const UTC: &str = "2004-07-01T00:00:00";
const UTC_2: &str = "2004-07-01T06:00:00";
const LSK: &str = "/kernels/naif0012.tls";
const SPK: &str = "/kernels/de440s-inner-cassini.bsp";

// The interpreter's own heap (IR, function tables, the stack), counted.
static HEAP_LIVE: AtomicUsize = AtomicUsize::new(0);
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);
static HEAP_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
// Guest linear memory, which SpaceWasm allocates outside that heap.
static LINEAR_LIVE: AtomicUsize = AtomicUsize::new(0);
static LINEAR_PEAK: AtomicUsize = AtomicUsize::new(0);

struct CountingHeap;

// SAFETY: forwards to the Rust global allocator, which upholds the same contract.
unsafe impl spacewasm::Allocator for CountingHeap {
    unsafe fn alloc(&self, layout: Layout) -> Result<*mut u8, spacewasm::AllocError> {
        if layout.size() == 0 {
            return Ok(layout.align() as *mut u8);
        }
        // SAFETY: non-zero size, checked above.
        let ptr = unsafe { std::alloc::alloc(layout) };
        if ptr.is_null() {
            return Err(spacewasm::AllocError::AllocationFailed);
        }
        let live = HEAP_LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        HEAP_PEAK.fetch_max(live, Ordering::Relaxed);
        HEAP_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        Ok(ptr)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if layout.size() == 0 || ptr.is_null() {
            return;
        }
        // SAFETY: `ptr` came from `alloc` with `layout`.
        unsafe { std::alloc::dealloc(ptr, layout) };
        HEAP_LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

spacewasm::global_allocator!(CountingHeap, CountingHeap);

struct LinearMemory;

impl LinearMemory {
    fn grew(old: usize, new: usize) {
        let live = LINEAR_LIVE.fetch_add(new, Ordering::Relaxed) + new - old;
        LINEAR_LIVE.fetch_sub(old, Ordering::Relaxed);
        LINEAR_PEAK.fetch_max(live, Ordering::Relaxed);
    }
}

impl WasmMemoryAllocator for LinearMemory {
    fn allocate(&self, layout: Layout) -> Result<NonNull<u8>, spacewasm::AllocError> {
        if layout.size() == 0 {
            return Ok(NonNull::dangling());
        }
        Self::grew(0, layout.size());
        // SAFETY: non-zero size, checked above.
        NonNull::new(unsafe { std::alloc::alloc_zeroed(layout) })
            .ok_or(spacewasm::AllocError::AllocationFailed)
    }

    fn reallocate(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        layout: Layout,
    ) -> Result<NonNull<u8>, spacewasm::AllocError> {
        if old_layout.size() == 0 {
            return self.allocate(layout);
        }
        Self::grew(old_layout.size(), layout.size());
        // SAFETY: `ptr` came from `allocate` or `reallocate` with `old_layout`.
        NonNull::new(unsafe { std::alloc::realloc(ptr.as_ptr(), old_layout, layout.size()) })
            .ok_or(spacewasm::AllocError::AllocationFailed)
    }

    fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            LINEAR_LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            // SAFETY: `ptr` came from `allocate` or `reallocate` with `layout`.
            unsafe { std::alloc::dealloc(ptr.as_ptr(), layout) }
        }
    }
}

/// The whole module handed to the decoder as one chunk.
struct Slice<'a> {
    bytes: &'a [u8],
    done: bool,
}

impl WasmStream for Slice<'_> {
    fn read(&mut self) -> Result<Option<InnerVec<u8>>, u8> {
        if self.done {
            return Ok(None);
        }
        self.done = true;
        // SAFETY: the decoder only reads through the view, and `self.bytes` outlives it.
        Ok(Some(unsafe {
            InnerVec::from_raw_parts(
                self.bytes.as_ptr().cast_mut(),
                self.bytes.len(),
                self.bytes.len(),
            )
        }))
    }

    fn return_(&mut self, _: InnerVec<u8>) {}
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

struct LoadStats {
    load_time: Duration,
    code_pages: usize,
    ir_words: usize,
    heap_after_load: usize,
    heap_allocations: usize,
    linear_bytes_at_load: usize,
    rss_after_load_kib: u64,
}

#[derive(Clone)]
struct CallStats {
    name: String,
    /// Calls to `Interpreter::run`, each given `slice` instructions of fuel.
    slices: u64,
    wall: Duration,
    wasi_calls: u64,
}

struct Host {
    engine: Engine,
    code: CodeBuilder,
    module: ModuleRef,
    wasi_calls: Rc<Cell<u64>>,
    exit_code: Rc<Cell<Option<i32>>>,
    slice: usize,
    log: Vec<CallStats>,
}

impl Host {
    fn load(bytes: &[u8], fixtures: &str, slice: usize, stack_words: usize) -> Res<(Host, LoadStats)> {
        let mut builder = WasiCtxBuilder::new();
        builder.arg("cspice.wasi.wasm")?;
        builder.inherit_stdio();
        let dir = Dir::open_ambient_dir(fixtures, ambient_authority())
            .map_err(|err| format!("could not open {fixtures}: {err}"))?;
        builder.preopened_dir(dir, "/kernels")?;
        let wasi = wasi::make_wasi_preview1_module(builder.build());
        let wasi_calls = Rc::clone(&wasi.calls);
        let exit_code = Rc::clone(&wasi.exit_code);

        let heap_before = HEAP_LIVE.load(Ordering::Relaxed);
        let allocations_before = HEAP_ALLOCATIONS.load(Ordering::Relaxed);
        let started = Instant::now();
        let mut code = CodeBuilder::new(CompilerOptions::new(true, None, MAX_CODE_PAGES))
            .map_err(|err| format!("could not allocate the code pages: {err:?}"))?;
        let mut engine = Engine::new(stack_words, 1, spacewasm::vec![wasi.module])
            .map_err(|err| format!("could not create the interpreter: {err:?}"))?;
        let memory = spacewasm::Rc::new(LinearMemory)
            .map_err(|err| format!("out of memory: {err:?}"))?
            .into_wasm_memory_allocator();
        let mut control_frames = vec![ControlFrame::EMPTY; DECODER_FRAMES];
        let mut operands = vec![OperandType::EMPTY; DECODER_DEPTH];
        let module = Module::new(
            "cspice",
            &mut Slice { bytes, done: false },
            &mut engine.store,
            &mut code,
            &mut DecoderStack::new(&mut control_frames, &mut operands),
            memory,
        )
        .map_err(|err| {
            format!("the module does not load at byte {:#x}: {:?}", err.offset, err.err.err)
        })?;
        let module = engine
            .push_module(module)
            .map_err(|err| format!("could not instantiate the module: {err:?}"))?;
        let load_time = started.elapsed();

        let pages = code.pages().len();
        let stats = LoadStats {
            load_time,
            code_pages: pages,
            ir_words: pages.saturating_sub(1) * 256 + code.offset(),
            heap_after_load: HEAP_LIVE.load(Ordering::Relaxed) - heap_before,
            heap_allocations: HEAP_ALLOCATIONS.load(Ordering::Relaxed) - allocations_before,
            linear_bytes_at_load: LINEAR_LIVE.load(Ordering::Relaxed),
            rss_after_load_kib: proc_status_kib("VmRSS:").unwrap_or(0),
        };
        let mut host = Host { engine, code, module, wasi_calls, exit_code, slice, log: Vec::new() };
        if let Some(start) = host.engine.module_start(module) {
            host.run("(start function)", start, &[])?;
        }
        Ok((host, stats))
    }

    fn export(&self, name: &str) -> Res<WasmRef> {
        let module = &self.engine.store.modules()[self.module.0 as usize];
        let export = module
            .exports
            .iter()
            .find(|export| export.name == name)
            .ok_or_else(|| format!("the module does not export `{name}`"))?;
        let ExportDesc::Func(index) = export.desc else {
            return Err(format!("`{name}` is not a function").into());
        };
        match module.get_func_ref(index) {
            Some(Ref::Module(index)) => Ok(WasmRef { module: self.module, index }),
            _ => Err(format!("`{name}` is not defined by the module").into()),
        }
    }

    /// Run one exported function to completion in slices of `self.slice` instructions.
    fn run(&mut self, label: &str, f: WasmRef, args: &[Value]) -> Res<Option<u64>> {
        let wasi_before = self.wasi_calls.get();
        let started = Instant::now();
        self.engine
            .invoke(f, args)
            .map_err(|err| format!("could not invoke {label}: {err:?}"))?;
        let mut slices = 0u64;
        loop {
            slices += 1;
            match Interpreter.run(self.code.pages(), &mut self.engine, self.slice) {
                InterpreterResult::OutOfFuel => continue,
                InterpreterResult::Finished => break,
                other => {
                    return Err(match self.exit_code.get() {
                        Some(code) => format!("{label}: the guest called proc_exit({code})"),
                        None => format!("{label} did not finish: {other:?}"),
                    }
                    .into());
                }
            }
        }
        self.log.push(CallStats {
            name: label.to_string(),
            slices,
            wall: started.elapsed(),
            wasi_calls: self.wasi_calls.get() - wasi_before,
        });
        Ok(self.engine.result.map(|raw| raw.read_64()))
    }

    fn call(&mut self, label: &str, name: &str, args: &[Value]) -> Res<Option<u64>> {
        let f = self.export(name)?;
        self.run(label, f, args)
    }

    fn write(&self, addr: u32, data: &[u8]) -> Res<()> {
        self.engine
            .store
            .get_memory(self.module)
            .store(addr as usize, data)
            .map_err(|err| format!("guest memory write at {addr:#x} failed: {err:?}").into())
    }

    fn read_f64(&self, addr: u32) -> Res<f64> {
        let bits = self
            .engine
            .store
            .get_memory(self.module)
            .load_u64(addr as usize)
            .map_err(|err| format!("guest memory read at {addr:#x} failed: {err:?}"))?;
        Ok(f64::from_bits(bits))
    }

    fn linear_bytes(&self) -> usize {
        self.engine.store.get_memory(self.module).get_slice().len()
    }

    /// Raise a readable error if CSPICE has signalled one.
    fn check(&mut self, what: &str, scratch: u32) -> Res<()> {
        if self.call("(failed_c)", "failed_c", &[])?.unwrap_or(0) as u32 == 0 {
            return Ok(());
        }
        let msg = scratch + 2048;
        self.write(scratch + 1024, b"LONG\0")?;
        self.call(
            "(getmsg_c)",
            "getmsg_c",
            &[Value::I32((scratch + 1024) as i32), Value::I32(1841), Value::I32(msg as i32)],
        )?;
        let memory = self.engine.store.get_memory(self.module);
        let raw = memory.load(msg as usize, 1841).map_err(|err| format!("{err:?}"))?;
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        Err(format!("{what}: {}", String::from_utf8_lossy(&raw[..end])).into())
    }
}

struct Outcome {
    load: LoadStats,
    calls: Vec<CallStats>,
    et: f64,
    state: [f64; 6],
    lt: f64,
    linear_bytes_end: usize,
}

/// Load the module on a fresh engine and run the measured sequence of SPICE calls.
fn scenario(
    bytes: &[u8],
    fixtures: &str,
    slice: usize,
    repeat: usize,
    stack_words: usize,
) -> Res<Outcome> {
    let (mut host, load) = Host::load(bytes, fixtures, slice, stack_words)?;
    host.call("_initialize", "_initialize", &[])?;

    // One guest allocation for every string and output the calls need.
    let scratch = host
        .call("malloc(8192)", "malloc", &[Value::I32(8192)])?
        .ok_or("malloc returned nothing")? as u32;
    if scratch == 0 {
        return Err("guest malloc failed".into());
    }
    let mut next = scratch;
    let mut cstr = |host: &Host, s: &str| -> Res<i32> {
        let at = next;
        let mut data = s.as_bytes().to_vec();
        data.push(0);
        host.write(at, &data)?;
        next += data.len() as u32;
        Ok(at as i32)
    };
    let set = cstr(&host, "SET")?;
    let ret = cstr(&host, "RETURN")?;
    let none_dev = cstr(&host, "NONE")?;
    let lsk = cstr(&host, LSK)?;
    let spk = cstr(&host, SPK)?;
    let utc = cstr(&host, UTC)?;
    let utc_2 = cstr(&host, UTC_2)?;
    let earth = cstr(&host, "EARTH")?;
    let sun = cstr(&host, "SUN")?;
    let j2000 = cstr(&host, "J2000")?;
    let lts = cstr(&host, "LT+S")?;
    let geometric = cstr(&host, "NONE")?;
    // Outputs live in the second half of the scratch block, 8-byte aligned.
    let out = (scratch + 4096 + 7) & !7;
    let out_i = Value::I32(out as i32);
    let lt_i = Value::I32((out + 48) as i32);
    let msg_scratch = scratch + 4096 + 256;

    host.call("erract_c(SET, RETURN)", "erract_c", &[Value::I32(set), Value::I32(0), Value::I32(ret)])?;
    host.call("errprt_c(SET, NONE)", "errprt_c", &[Value::I32(set), Value::I32(0), Value::I32(none_dev)])?;

    host.call("furnsh_c(naif0012.tls)", "furnsh_c", &[Value::I32(lsk)])?;
    host.check("furnsh_c(lsk)", msg_scratch)?;
    host.call("furnsh_c(de440s subset)", "furnsh_c", &[Value::I32(spk)])?;
    host.check("furnsh_c(spk)", msg_scratch)?;

    host.call("str2et_c, first call", "str2et_c", &[Value::I32(utc_2), out_i])?;
    host.check("str2et_c", msg_scratch)?;
    host.call("str2et_c, second call", "str2et_c", &[Value::I32(utc), out_i])?;
    host.check("str2et_c", msg_scratch)?;
    let et = host.read_f64(out)?;

    let spkezr = |host: &mut Host, label: &str, t: f64, abcorr: i32| -> Res<()> {
        host.call(
            label,
            "spkezr_c",
            &[
                Value::I32(earth),
                Value::F64(t),
                Value::I32(j2000),
                Value::I32(abcorr),
                Value::I32(sun),
                out_i,
                lt_i,
            ],
        )?;
        Ok(())
    };

    // First call: body name lookups, frame setup, segment search, and file reads.
    spkezr(&mut host, "spkezr_c LT+S, first call", et + 7200.0, lts)?;
    host.check("spkezr_c", msg_scratch)?;
    // Steady state: new epochs inside the already buffered segment records.
    for k in 0..repeat {
        spkezr(&mut host, "spkezr_c LT+S, steady state", et + 7200.0 + 600.0 * (k + 1) as f64, lts)?;
    }
    host.check("spkezr_c", msg_scratch)?;
    for k in 0..repeat {
        spkezr(&mut host, "spkezr_c NONE, steady state", et + 7200.0 + 600.0 * (k + 1) as f64, geometric)?;
    }
    host.check("spkezr_c", msg_scratch)?;

    // The reference call, last, so its outputs are what gets printed and compared.
    spkezr(&mut host, "spkezr_c LT+S, reference epoch", et, lts)?;
    host.check("spkezr_c", msg_scratch)?;
    let mut state = [0.0; 6];
    for (i, v) in state.iter_mut().enumerate() {
        *v = host.read_f64(out + 8 * i as u32)?;
    }
    let lt = host.read_f64(out + 48)?;

    // Last, so it perturbs nothing above: the same text kernel again, which separates
    // the cost of parsing it from the one-time initialization the first load pays.
    host.call("furnsh_c(naif0012.tls), again", "furnsh_c", &[Value::I32(lsk)])?;
    host.check("furnsh_c(lsk), again", msg_scratch)?;

    Ok(Outcome {
        linear_bytes_end: host.linear_bytes(),
        load,
        calls: host.log,
        et,
        state,
        lt,
    })
}

/// Collapse repeated labels (the steady-state loops, the error checks) into one row.
struct Row {
    name: String,
    count: u64,
    slices: u64,
    wall: Duration,
    wasi_calls: u64,
}

fn rows(calls: &[CallStats]) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for call in calls {
        match out.iter_mut().find(|row| row.name == call.name) {
            Some(row) => {
                row.count += 1;
                row.slices += call.slices;
                row.wall += call.wall;
                row.wasi_calls += call.wasi_calls;
            }
            None => out.push(Row {
                name: call.name.clone(),
                count: 1,
                slices: call.slices,
                wall: call.wall,
                wasi_calls: call.wasi_calls,
            }),
        }
    }
    out
}

fn proc_status_kib(key: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with(key))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn main() -> Res<()> {
    let mut module_path = None;
    let mut fixtures = None;
    let mut json_path = None;
    let mut slice = 1000usize;
    let mut repeat = 20usize;
    let mut skip_exact = false;
    let mut stack_words = DEFAULT_STACK_WORDS;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--module" => module_path = Some(value("--module")?),
            "--fixtures" => fixtures = Some(value("--fixtures")?),
            "--json" => json_path = Some(value("--json")?),
            "--slice" => slice = value("--slice")?.parse()?,
            "--repeat" => repeat = value("--repeat")?.parse()?,
            "--stack-words" => stack_words = value("--stack-words")?.parse()?,
            "--skip-exact" => skip_exact = true,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let module_path = module_path.ok_or("--module <cspice.wasi.wasm> is required")?;
    let fixtures = fixtures.ok_or("--fixtures <dir> is required")?;
    let bytes = std::fs::read(&module_path).map_err(|err| format!("{module_path}: {err}"))?;

    // Pass 1: unlimited fuel, for wall time without slicing overhead. It runs first so
    // the resident-set figures describe one engine, before the other passes add theirs.
    let rss_start = proc_status_kib("VmRSS:").unwrap_or(0);
    let free_run = scenario(&bytes, &fixtures, usize::MAX, repeat, stack_words)?;
    let rss_one_engine = proc_status_kib("VmHWM:").unwrap_or(0);
    let heap_peak_one_engine = HEAP_PEAK.load(Ordering::Relaxed);
    let linear_peak_one_engine = LINEAR_PEAK.load(Ordering::Relaxed);

    // Pass 2: fixed slices, as a sequencer would schedule the guest.
    let sliced = scenario(&bytes, &fixtures, slice, repeat, stack_words)?;
    // Pass 3: one instruction per slice, so the slice count is the instruction count.
    let exact = if skip_exact { None } else { Some(scenario(&bytes, &fixtures, 1, repeat, stack_words)?) };

    let load = &free_run.load;
    println!("module {module_path}: {} bytes", bytes.len());
    println!("interpreter spacewasm {}", "0.8.0-rc.3");
    println!(
        "load (decode, validate, compile, instantiate): {:.1} ms",
        load.load_time.as_secs_f64() * 1e3
    );
    println!(
        "compiled IR: {} 16-bit words ({} bytes) in {} code pages of 512 bytes; {:.2} IR bytes per module byte",
        load.ir_words,
        load.ir_words * 2,
        load.code_pages,
        (load.ir_words * 2) as f64 / bytes.len() as f64
    );
    println!(
        "interpreter heap after load: {} bytes in {} allocations (includes the {} byte operand stack)",
        load.heap_after_load,
        load.heap_allocations,
        stack_words * 4
    );
    println!("interpreter heap peak, one engine: {heap_peak_one_engine} bytes");
    println!(
        "guest linear memory: {} bytes at load, {} bytes after the calls, peak {} bytes",
        load.linear_bytes_at_load, free_run.linear_bytes_end, linear_peak_one_engine
    );
    println!(
        "host process resident set: {rss_start} KiB before load, {} KiB after load, peak {rss_one_engine} KiB after one engine ran the scenario",
        load.rss_after_load_kib
    );
    println!();

    let free_rows = rows(&free_run.calls);
    let sliced_rows = rows(&sliced.calls);
    let exact_rows = exact.as_ref().map(|outcome| rows(&outcome.calls));
    println!(
        "{:<34} {:>5} {:>14} {:>12} {:>12} {:>12} {:>6}",
        "call (per call where n > 1)", "n", "instructions", "slices@".to_string() + &slice.to_string(), "ms free-run", "ms sliced", "wasi"
    );
    let mut json_rows = String::new();
    for (i, row) in free_rows.iter().enumerate() {
        if row.name.starts_with('(') && row.name != "(start function)" {
            continue;
        }
        let n = row.count as f64;
        let instructions = exact_rows.as_ref().map(|rows| rows[i].slices as f64 / n);
        let slices = sliced_rows[i].slices as f64 / n;
        let ms_free = row.wall.as_secs_f64() * 1e3 / n;
        let ms_sliced = sliced_rows[i].wall.as_secs_f64() * 1e3 / n;
        let wasi = row.wasi_calls as f64 / n;
        println!(
            "{:<34} {:>5} {:>14} {:>12.1} {:>12.3} {:>12.3} {:>6.1}",
            row.name,
            row.count,
            instructions.map_or("n/a".to_string(), |v| format!("{v:.0}")),
            slices,
            ms_free,
            ms_sliced,
            wasi
        );
        if !json_rows.is_empty() {
            json_rows.push_str(",\n");
        }
        write!(
            json_rows,
            "    {{\"call\": \"{}\", \"n\": {}, \"instructions\": {}, \"slices\": {:.1}, \"ms_free_run\": {:.4}, \"ms_sliced\": {:.4}, \"wasi_calls\": {:.1}}}",
            row.name,
            row.count,
            instructions.map_or("null".to_string(), |v| format!("{v:.0}")),
            slices,
            ms_free,
            ms_sliced,
            wasi
        )?;
    }
    println!();

    // The numbers themselves, for the smoke test to hold against the Emscripten oracle.
    let s = &free_run.state;
    println!("et {:e}", free_run.et);
    println!("state {:e} {:e} {:e} {:e} {:e} {:e}", s[0], s[1], s[2], s[3], s[4], s[5]);
    println!("lt {:e}", free_run.lt);
    for other in [Some(&sliced), exact.as_ref()].into_iter().flatten() {
        if other.et.to_bits() != free_run.et.to_bits()
            || other.lt.to_bits() != free_run.lt.to_bits()
            || other.state.iter().zip(s).any(|(a, b)| a.to_bits() != b.to_bits())
        {
            return Err("the passes disagree on the computed values".into());
        }
    }

    if let Some(path) = json_path {
        let json = format!(
            "{{\n  \"module_bytes\": {},\n  \"spacewasm\": \"0.8.0-rc.3\",\n  \"slice\": {},\n  \"load_ms\": {:.2},\n  \"ir_words\": {},\n  \"ir_bytes\": {},\n  \"code_pages\": {},\n  \"heap_after_load_bytes\": {},\n  \"heap_allocations\": {},\n  \"heap_peak_bytes\": {},\n  \"operand_stack_bytes\": {},\n  \"linear_memory_bytes_at_load\": {},\n  \"linear_memory_bytes_end\": {},\n  \"rss_before_load_kib\": {},\n  \"rss_after_load_kib\": {},\n  \"rss_peak_one_engine_kib\": {},\n  \"calls\": [\n{}\n  ]\n}}\n",
            bytes.len(),
            slice,
            load.load_time.as_secs_f64() * 1e3,
            load.ir_words,
            load.ir_words * 2,
            load.code_pages,
            load.heap_after_load,
            load.heap_allocations,
            heap_peak_one_engine,
            stack_words * 4,
            load.linear_bytes_at_load,
            free_run.linear_bytes_end,
            rss_start,
            load.rss_after_load_kib,
            rss_one_engine,
            json_rows
        );
        std::fs::write(&path, json).map_err(|err| format!("{path}: {err}"))?;
    }
    Ok(())
}
