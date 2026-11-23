use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use shader_graph::codegen::RenderPlan;

use crate::renderer::{Compiler, Program, ProgramDiagnostic, ProgramError};

struct Job {
    id: u64,
    plan: RenderPlan,
}

#[derive(Default)]
struct Slot {
    pending: Option<Job>,
    closed: bool,
}

type Shared = (Mutex<Slot>, Condvar);

/// A plan's compile, finished.
pub struct Compiled {
    /// What `CompileQueue::submit` returned for the plan.
    pub id: u64,
    pub result: Result<Program, ProgramError>,
}

/// Compiles plans on a thread of its own, one at a time. Plans submitted while one compiles
/// replace each other: when it finishes, only the latest is compiled, so the results that come
/// out skip the ones overtaken, and the last submitted plan always comes out.
pub struct CompileQueue {
    shared: Arc<Shared>,
    done: Receiver<Compiled>,
    sent: u64,
    received: u64,
}

impl CompileQueue {
    /// `wake` is called from the thread each time a result is ready.
    pub fn new(compiler: Compiler, wake: impl Fn() + Send + 'static) -> Self {
        Self::with_compile(move |plan| compiler.compile(plan), wake)
    }

    fn with_compile(
        compile: impl Fn(&RenderPlan) -> Result<Program, ProgramError> + Send + 'static,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let shared: Arc<Shared> = Arc::default();
        let (sender, done) = channel();
        let worker = shared.clone();
        // If the thread doesn't start or stops, its sender is dropped and `done` disconnects; the
        // queue then reports the plan it was left with as an error.
        let spawned = std::thread::Builder::new().name("shader compile".into()).spawn(move || {
            loop {
                let job = {
                    let (slot, signal) = &*worker;
                    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
                    loop {
                        if slot.closed {
                            return;
                        }
                        if let Some(job) = slot.pending.take() {
                            break job;
                        }
                        slot = signal.wait(slot).unwrap_or_else(PoisonError::into_inner);
                    }
                };
                let result = catch_unwind(AssertUnwindSafe(|| compile(&job.plan))).unwrap_or_else(|_| {
                    Err(ProgramError {
                        diagnostics: vec![ProgramDiagnostic { message: "the shader compiler crashed".to_owned(), location: None }],
                        code: String::new(),
                    })
                });
                if sender.send(Compiled { id: job.id, result }).is_err() {
                    return;
                }
                wake();
            }
        });
        if let Err(error) = spawned {
            log::error!("the shader compile thread didn't start: {error}");
        }
        Self { shared, done, sent: 0, received: 0 }
    }

    /// Queues a plan, replacing the one waiting if it hasn't started. Returns its id.
    pub fn submit(&mut self, plan: RenderPlan) -> u64 {
        self.sent += 1;
        let (slot, signal) = &*self.shared;
        slot.lock().unwrap_or_else(PoisonError::into_inner).pending = Some(Job { id: self.sent, plan });
        signal.notify_one();
        self.sent
    }

    /// The id of the plan submitted last.
    pub fn latest(&self) -> u64 {
        self.sent
    }

    /// Whether the plan submitted last hasn't come out yet.
    pub fn busy(&self) -> bool {
        self.received < self.sent
    }

    /// The next finished compile, if there is one; never waits.
    pub fn try_next(&mut self) -> Option<Compiled> {
        match self.done.try_recv() {
            Ok(done) => {
                self.received = done.id;
                Some(done)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => self.busy().then(|| self.stopped()),
        }
    }

    /// The next finished compile, waiting for it; `None` when nothing is queued.
    pub fn wait_next(&mut self) -> Option<Compiled> {
        if !self.busy() {
            return None;
        }
        match self.done.recv() {
            Ok(done) => {
                self.received = done.id;
                Some(done)
            }
            Err(_) => Some(self.stopped()),
        }
    }

    /// The result of the plan still waiting once the compile thread is gone: it can't come out.
    fn stopped(&mut self) -> Compiled {
        self.received = self.sent;
        Compiled {
            id: self.sent,
            result: Err(ProgramError {
                diagnostics: vec![ProgramDiagnostic { message: "the shader compile thread stopped".to_owned(), location: None }],
                code: String::new(),
            }),
        }
    }
}

impl Drop for CompileQueue {
    fn drop(&mut self) {
        let (slot, signal) = &*self.shared;
        slot.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
        signal.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::sync_channel;

    use super::*;
    use crate::renderer::Renderer;
    use crate::renderer::tests::{CODE, plan};

    fn empty_plan() -> RenderPlan {
        RenderPlan { resources: Vec::new(), passes: Vec::new(), uniforms: Vec::new(), frame: Default::default() }
    }

    fn plan_marked(n: usize) -> RenderPlan {
        let mut plan = empty_plan();
        plan.uniforms = vec![(format!("u{n}"), shader_graph::codegen::UniformValue(vec![]))];
        plan
    }

    fn failing(plan: &RenderPlan) -> Result<Program, ProgramError> {
        Err(ProgramError {
            diagnostics: vec![ProgramDiagnostic { message: plan.uniforms[0].0.clone(), location: None }],
            code: String::new(),
        })
    }

    #[test]
    fn submit_returns_while_the_compile_is_blocked_and_the_result_comes_out_later() {
        let (release, gate) = sync_channel::<()>(0);
        let gate = Mutex::new(gate);
        let mut queue = CompileQueue::with_compile(
            move |plan| {
                let _ = gate.lock().unwrap().recv();
                failing(plan)
            },
            || {},
        );
        let started = std::time::Instant::now();
        queue.submit(plan_marked(1));
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert!(queue.busy());
        assert!(queue.try_next().is_none());
        release.send(()).unwrap();
        let done = queue.wait_next().expect("a result");
        assert_eq!(done.id, 1);
        assert!(!queue.busy());
        assert!(queue.wait_next().is_none());
    }

    #[test]
    fn plans_submitted_while_one_compiles_are_coalesced_into_the_latest() {
        let (release, gate) = sync_channel::<()>(0);
        let (started, running) = sync_channel::<()>(0);
        let gate = Mutex::new(gate);
        let compiled = Arc::new(AtomicUsize::new(0));
        let count = compiled.clone();
        let mut queue = CompileQueue::with_compile(
            move |plan| {
                count.fetch_add(1, Ordering::SeqCst);
                let _ = started.send(());
                let _ = gate.lock().unwrap().recv();
                failing(plan)
            },
            || {},
        );
        queue.submit(plan_marked(1));
        running.recv().unwrap();
        queue.submit(plan_marked(2));
        queue.submit(plan_marked(3));
        queue.submit(plan_marked(4));
        release.send(()).unwrap();
        let first = queue.wait_next().expect("the first");
        assert_eq!(first.id, 1);
        assert!(queue.busy());
        running.recv().unwrap();
        release.send(()).unwrap();
        let last = queue.wait_next().expect("the latest");
        assert_eq!(last.id, 4);
        assert_eq!(last.result.err().unwrap().diagnostics[0].message, "u4");
        assert_eq!(compiled.load(Ordering::SeqCst), 2, "the plans in between were never compiled");
        assert!(!queue.busy());
    }

    #[test]
    fn a_panicking_compile_comes_out_as_an_error() {
        let mut queue = CompileQueue::with_compile(|_| panic!("boom"), || {});
        queue.submit(empty_plan());
        let done = queue.wait_next().expect("a result");
        assert!(done.result.is_err());
        queue.submit(empty_plan());
        assert!(queue.wait_next().is_some(), "the thread keeps going");
    }

    #[test]
    fn a_plan_left_behind_by_a_stopped_thread_comes_out_as_an_error() {
        let mut queue = CompileQueue::with_compile(failing, || panic!("the waker panics"));
        queue.submit(plan_marked(1));
        assert_eq!(queue.wait_next().expect("the first").id, 1);
        queue.submit(plan_marked(2));
        let stopped = queue.wait_next().expect("a result for the second");
        assert_eq!(stopped.id, 2);
        assert!(stopped.result.is_err());
        assert!(!queue.busy());
    }

    #[test]
    fn the_device_compiles_off_thread() {
        let renderer = Renderer::new().unwrap();
        let mut queue = CompileQueue::new(renderer.compiler(), || {});
        queue.submit(plan(CODE));
        let done = queue.wait_next().expect("a result");
        assert!(done.result.is_ok(), "{:?}", done.result.err());
    }
}
