//! ToolRuntime adapter: runs tool code as a WASM module in wasmtime (ADR-0014, ADR-0029).
//!
//! Tool ABI v1: core module without imports, with the exports `memory`, `gw_alloc` and
//! `gw_call`; input and output as UTF-8 JSON. Every call runs in a fresh instance with a fuel
//! and memory limit, and optionally a time limit; a tool package can ask for stricter limits
//! per tool (ADR-0124).

use std::sync::{Arc, Once};
use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_tools::{DynTool, ToolArtifact, ToolDescriptor, ToolError, ToolLimits, ToolRuntime};
use serde_json::Value;
use wasmtime::{Config, Engine, Instance, Module, Store, StoreLimits, StoreLimitsBuilder};

/// Limits for every tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Compute budget (wasmtime fuel) per call.
    pub fuel: u64,
    /// Maximum size of the linear memory in bytes.
    pub max_memory_bytes: usize,
    /// Wall-clock time per call; `None`: no time limit.
    pub time: Option<Duration>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel: 100_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
            time: None,
        }
    }
}

impl Limits {
    /// The stricter of these limits and the ones a package asks for (ADR-0124).
    pub fn stricter(&self, package: &ToolLimits) -> Self {
        let memory = package
            .memory_mib
            .map(|mib| usize::try_from(mib.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX));
        let time = package.time_ms.map(Duration::from_millis);
        Self {
            fuel: package.fuel.map_or(self.fuel, |fuel| fuel.min(self.fuel)),
            max_memory_bytes: memory.map_or(self.max_memory_bytes, |memory| {
                memory.min(self.max_memory_bytes)
            }),
            time: match (self.time, time) {
                (Some(host), Some(package)) => Some(host.min(package)),
                (host, package) => host.or(package),
            },
        }
    }
}

/// How often the epoch of an engine advances: the granularity of time limits.
const TICK: Duration = Duration::from_millis(5);

/// Loads tool artifacts into a wasmtime sandbox.
#[derive(Clone)]
pub struct SandboxRuntime {
    engine: Engine,
    limits: Limits,
    /// Starts the epoch ticker once, when the first tool with a time limit is loaded.
    ticker: Arc<Once>,
}

impl SandboxRuntime {
    /// Sandbox with default limits.
    pub fn new() -> Result<Self, ToolError> {
        Self::with_limits(Limits::default())
    }

    /// Sandbox with custom limits.
    pub fn with_limits(limits: Limits) -> Result<Self, ToolError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(execution)?;
        Ok(Self {
            engine,
            limits,
            ticker: Arc::new(Once::new()),
        })
    }

    fn tool(
        &self,
        artifact: &ToolArtifact,
        descriptor: ToolDescriptor,
        limits: Limits,
    ) -> Result<Arc<dyn DynTool>, ToolError> {
        let module = Module::new(&self.engine, &artifact.bytes).map_err(execution)?;
        let imports: Vec<String> = module
            .imports()
            .map(|import| format!("{}::{}", import.module(), import.name()))
            .collect();
        if !imports.is_empty() {
            return Err(ToolError::Execution(format!(
                "tool '{}' imports {} – tool ABI v1 allows no imports (ADR-0029)",
                artifact.name,
                imports.join(", ")
            )));
        }
        if limits.time.is_some() {
            self.start_ticker();
        }
        Ok(Arc::new(SandboxTool {
            engine: self.engine.clone(),
            module,
            descriptor,
            limits,
        }))
    }

    /// Advances the epoch of the engine every [`TICK`] for as long as the engine lives, once
    /// per engine: time limits are counted in epochs.
    fn start_ticker(&self) {
        let engine = self.engine.weak();
        self.ticker.call_once(|| {
            std::thread::spawn(move || {
                while let Some(engine) = engine.upgrade() {
                    engine.increment_epoch();
                    drop(engine);
                    std::thread::sleep(TICK);
                }
            });
        });
    }
}

impl ToolRuntime for SandboxRuntime {
    fn instantiate(
        &self,
        artifact: &ToolArtifact,
        descriptor: ToolDescriptor,
    ) -> Result<Arc<dyn DynTool>, ToolError> {
        self.tool(artifact, descriptor, self.limits)
    }

    fn instantiate_limited(
        &self,
        artifact: &ToolArtifact,
        descriptor: ToolDescriptor,
        limits: &ToolLimits,
    ) -> Result<Arc<dyn DynTool>, ToolError> {
        self.tool(artifact, descriptor, self.limits.stricter(limits))
    }
}

/// A tool whose code runs in the sandbox.
struct SandboxTool {
    engine: Engine,
    module: Module,
    descriptor: ToolDescriptor,
    limits: Limits,
}

impl SandboxTool {
    /// Executes a call in a fresh instance.
    fn run(&self, args: &Value) -> Result<String, ToolError> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.limits.max_memory_bytes)
            .build();
        let mut store: Store<StoreLimits> = Store::new(&self.engine, limits);
        store.limiter(|limits| limits);
        store.set_fuel(self.limits.fuel).map_err(execution)?;
        match self.limits.time {
            Some(time) => {
                let ticks = time.as_nanos().div_ceil(TICK.as_nanos()).max(1) as u64;
                // One more: the first tick may come right away.
                store.set_epoch_deadline(ticks + 1);
            }
            None => store.set_epoch_deadline(u64::MAX / 2),
        }

        let instance = Instance::new(&mut store, &self.module, &[]).map_err(|error| {
            let text = error.to_string();
            if text.contains("memory") || text.contains("resource limit") {
                ToolError::Execution(format!(
                    "the tool needs more memory than its limit ({} bytes): {text}",
                    self.limits.max_memory_bytes
                ))
            } else {
                execution(error)
            }
        })?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| ToolError::Execution("module exports no memory".into()))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "gw_alloc")
            .map_err(execution)?;
        let call = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "gw_call")
            .map_err(execution)?;

        let input = serde_json::to_vec(args).map_err(execution)?;
        let input_len = i32::try_from(input.len()).map_err(execution)?;
        let input_ptr = alloc.call(&mut store, input_len).map_err(execution)?;
        memory
            .write(&mut store, offset(input_ptr)?, &input)
            .map_err(execution)?;

        let packed = call
            .call(&mut store, (input_ptr, input_len))
            .map_err(|error| call_error(error, self.limits.time))? as u64;
        let (output_ptr, output_len) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
        let mut output = vec![0; output_len];
        memory
            .read(&store, output_ptr, &mut output)
            .map_err(execution)?;

        let output: Value = serde_json::from_slice(&output).map_err(execution)?;
        match (output.get("ok"), output.get("err")) {
            (Some(Value::String(text)), _) => Ok(text.clone()),
            (_, Some(Value::String(error))) => Err(ToolError::Execution(error.clone())),
            _ => Err(ToolError::Execution(format!(
                "invalid tool answer: {output}"
            ))),
        }
    }
}

impl DynTool for SandboxTool {
    fn name(&self) -> &str {
        &self.descriptor.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        let result = self.run(&args);
        Box::pin(async move { result })
    }
}

fn offset(pointer: i32) -> Result<usize, ToolError> {
    usize::try_from(pointer).map_err(execution)
}

fn execution(error: impl std::fmt::Display) -> ToolError {
    ToolError::Execution(error.to_string())
}

/// Translates errors while executing the tool code; an exhausted fuel limit is reported in a
/// way the LLM understands.
fn call_error(error: wasmtime::Error, time: Option<Duration>) -> ToolError {
    match error.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::OutOfFuel) => ToolError::Execution(
            "the tool's compute budget is exhausted (fuel limit) – probably an endless loop".into(),
        ),
        Some(wasmtime::Trap::Interrupt) => ToolError::Execution(format!(
            "the tool ran longer than its time limit ({} ms) and was stopped",
            time.map_or(0, |time| time.as_millis())
        )),
        _ => execution(error),
    }
}
