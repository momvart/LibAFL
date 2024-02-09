//! A `QEMU`-based executor for binary-only instrumentation in `LibAFL`
use core::{
    ffi::c_void,
    fmt::{self, Debug, Formatter},
    time::Duration,
};

#[cfg(feature = "fork")]
use libafl::{
    events::EventManager,
    executors::InProcessForkExecutor,
    state::{HasLastReportTime, HasMetadata},
};
use libafl::{
    events::{EventFirer, EventRestarter},
    executors::{
        hooks::inprocess::InProcessExecutorHandlerData,
        inprocess::{HasInProcessHooks, InProcessExecutor},
        Executor, ExitKind, HasExecutorState, HasObservers, NopExecutorState,
    },
    feedbacks::Feedback,
    fuzzer::HasObjective,
    observers::{ObserversTuple, UsesObservers},
    state::{HasCorpus, HasExecutions, HasSolutions, State, UsesState},
    Error,
};
use libafl_bolts::os::unix_signals::{siginfo_t, ucontext_t, Signal};
#[cfg(feature = "fork")]
use libafl_bolts::shmem::ShMemProvider;

use crate::{emu::Emulator, helper::QemuHelperTuple, hooks::QemuHooks};

pub struct QemuExecutorState<'a, QT, S>
where
    QT: QemuHelperTuple<S>,
    S: State + HasExecutions,
{
    hooks: &'a mut QemuHooks<QT, S>,
    first_exec: bool,
}

impl<'a, QT, S> HasExecutorState for QemuExecutorState<'a, QT, S>
where
    QT: QemuHelperTuple<S>,
    S: State + HasExecutions,
{
    type ExecutorState = Self;
}

pub struct QemuExecutor<'a, H, OT, QT, S>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<'a, QT, S>) -> ExitKind,
    S: State + HasExecutions,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
{
    inner: InProcessExecutor<'a, H, OT, S, QemuExecutorState<'a, QT, S>>,
    state: QemuExecutorState<'a, QT, S>,
}

impl<'a, H, OT, QT, S> Debug for QemuExecutor<'a, H, OT, QT, S>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<QT, S>) -> ExitKind,
    S: State + HasExecutions,
    OT: ObserversTuple<S> + Debug,
    QT: QemuHelperTuple<S> + Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("QemuExecutor")
            .field("hooks", &self.state.hooks)
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(emulation_mode = "usermode")]
extern "C" {
    // Original QEMU user signal handler
    fn libafl_qemu_handle_crash(signal: i32, info: *mut siginfo_t, puc: *mut c_void);
}

#[cfg(emulation_mode = "usermode")]
pub unsafe fn inproc_qemu_crash_handler<'a, E, EM, OF, Z, QT, S>(
    signal: Signal,
    info: &'a mut siginfo_t,
    mut context: Option<&'a mut ucontext_t>,
    _data: &'a mut InProcessExecutorHandlerData,
) where
    E: Executor<EM, Z, QemuExecutorState<'a, QT, S>> + HasObservers,
    EM: EventFirer<State = E::State> + EventRestarter<State = E::State>,
    OF: Feedback<E::State>,
    E::State: HasExecutions + HasSolutions + HasCorpus,
    Z: HasObjective<Objective = OF, State = E::State>,
    QT: QemuHelperTuple<S> + Debug + 'a,
    S: State + HasExecutions + 'a,
{
    let puc = match &mut context {
        Some(v) => (*v) as *mut ucontext_t as *mut c_void,
        None => core::ptr::null_mut(),
    };
    libafl_qemu_handle_crash(signal as i32, info, puc);
}

#[cfg(emulation_mode = "systemmode")]
static mut BREAK_ON_TMOUT: bool = false;

#[cfg(emulation_mode = "systemmode")]
extern "C" {
    fn qemu_system_debug_request();
}

#[cfg(emulation_mode = "systemmode")]
pub unsafe fn inproc_qemu_timeout_handler<'a, E, EM, OF, Z, QT, S>(
    signal: Signal,
    info: &'a mut siginfo_t,
    context: Option<&'a mut ucontext_t>,
    data: &'a mut InProcessExecutorHandlerData,
) where
    E: Executor<EM, Z, QemuExecutorState<'a, QT, S>> + HasObservers + HasInProcessHooks,
    EM: EventFirer<State = E::State> + EventRestarter<State = E::State>,
    OF: Feedback<E::State>,
    E::State: HasSolutions + HasCorpus + HasExecutions,
    Z: HasObjective<Objective = OF, State = E::State>,
    QT: QemuHelperTuple<S> + Debug + 'a,
    S: State + HasExecutions + 'a,
{
    if BREAK_ON_TMOUT {
        qemu_system_debug_request();
    } else {
        libafl::executors::hooks::unix::unix_signal_handler::inproc_timeout_handler::<E, EM, OF, Z>(
            signal, info, context, data,
        );
    }
}

impl<'a, H, OT, QT, S> QemuExecutor<'a, H, OT, QT, S>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<QT, S>) -> ExitKind,
    S: State + HasExecutions,
    OT: ObserversTuple<S> + Debug,
    QT: QemuHelperTuple<S> + Debug,
{
    pub fn new<EM, OF, Z>(
        hooks: &'a mut QemuHooks<QT, S>,
        harness_fn: &'a mut H,
        observers: OT,
        fuzzer: &mut Z,
        state: &mut S,
        event_mgr: &mut EM,
        timeout: Duration,
    ) -> Result<Self, Error>
    where
        EM: EventFirer<State = S> + EventRestarter<State = S>,
        OF: Feedback<S>,
        S: State + HasExecutions + HasCorpus + HasSolutions,
        Z: HasObjective<Objective = OF, State = S>,
    {
        let mut inner = InProcessExecutor::with_timeout(
            harness_fn, observers, fuzzer, state, event_mgr, timeout,
        )?;
        #[cfg(emulation_mode = "usermode")]
        {
            inner.inprocess_hooks_mut().crash_handler = inproc_qemu_crash_handler::<
                InProcessExecutor<'a, H, OT, S, QemuExecutorState<QT, S>>,
                EM,
                OF,
                Z,
                QT,
                S,
            > as *const c_void;

            let handler = |hooks: &mut QemuHooks<QT, S>, host_sig| {
                eprintln!("Crashed with signal {host_sig}");
                unsafe {
                    libafl::executors::inprocess::generic_inproc_crash_handler::<
                        InProcessExecutor<'a, H, OT, S, QemuExecutorState<QT, S>>,
                        EM,
                        OF,
                        Z,
                        QemuExecutorState<QT, S>,
                    >();
                }
                if let Some(cpu) = hooks.emulator().current_cpu() {
                    eprint!("Context:\n{}", cpu.display_context());
                }
            };

            hooks.crash_closure(Box::new(handler));
        }
        #[cfg(emulation_mode = "systemmode")]
        {
            inner.inprocess_hooks_mut().timeout_handler = inproc_qemu_timeout_handler::<
                InProcessExecutor<'a, H, OT, S, QemuExecutorState<QT, S>>,
                EM,
                OF,
                Z,
                QT,
                S,
            > as *const c_void;
        }
        Ok(Self {
            inner,
            state: QemuExecutorState {
                first_exec: true,
                hooks,
            },
        })
    }

    pub fn inner(&'a self) -> &InProcessExecutor<'a, H, OT, S, QemuExecutorState<QT, S>> {
        &self.inner
    }

    #[cfg(emulation_mode = "systemmode")]
    pub fn break_on_timeout(&mut self) {
        unsafe {
            BREAK_ON_TMOUT = true;
        }
    }

    pub fn inner_mut(
        &'a mut self,
    ) -> &mut InProcessExecutor<'a, H, OT, S, QemuExecutorState<QT, S>> {
        &mut self.inner
    }

    pub fn hooks(&self) -> &QemuHooks<QT, S> {
        self.state.hooks
    }

    pub fn hooks_mut(&mut self) -> &mut QemuHooks<QT, S> {
        self.state.hooks
    }

    pub fn emulator(&self) -> &Emulator {
        self.state.hooks.emulator()
    }
}

impl<'a, EM, H, OT, QT, S, Z> Executor<EM, Z, NopExecutorState> for QemuExecutor<'a, H, OT, QT, S>
where
    EM: UsesState<State = S>,
    H: FnMut(&S::Input, &mut QemuExecutorState<QT, S>) -> ExitKind,
    S: State + HasExecutions,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
    Z: UsesState<State = S>,
{
    fn run_target(
        &mut self,
        fuzzer: &mut Z,
        state: &mut Self::State,
        mgr: &mut EM,
        input: &Self::Input,
        _executor_state: &mut (),
    ) -> Result<ExitKind, Error> {
        let emu = Emulator::get().unwrap();
        if self.state.first_exec {
            self.state.hooks.helpers().first_exec_all(self.state.hooks);
            self.state.first_exec = false;
        }
        self.state.hooks.helpers_mut().pre_exec_all(&emu, input);
        let mut exit_kind = self
            .inner
            .run_target(fuzzer, state, mgr, input, &mut self.state)?;
        self.state.hooks.helpers_mut().post_exec_all(
            &emu,
            input,
            self.inner.observers_mut(),
            &mut exit_kind,
        );
        Ok(exit_kind)
    }
}

impl<'a, H, OT, QT, S> UsesState for QemuExecutor<'a, H, OT, QT, S>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<QT, S>) -> ExitKind,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
    S: State + HasExecutions,
{
    type State = S;
}

impl<'a, H, OT, QT, S> UsesObservers for QemuExecutor<'a, H, OT, QT, S>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<QT, S>) -> ExitKind,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
    S: State + HasExecutions,
{
    type Observers = OT;
}

impl<'a, H, OT, QT, S> HasObservers for QemuExecutor<'a, H, OT, QT, S>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<QT, S>) -> ExitKind,
    S: State + HasExecutions,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
{
    #[inline]
    fn observers(&self) -> &OT {
        self.inner.observers()
    }

    #[inline]
    fn observers_mut(&mut self) -> &mut OT {
        self.inner.observers_mut()
    }
}

#[cfg(feature = "fork")]
pub struct QemuForkExecutor<'a, H, OT, QT, S, SP>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<'a, QT, S>) -> ExitKind,
    S: State + HasExecutions,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
    SP: ShMemProvider,
{
    inner: InProcessForkExecutor<'a, H, OT, S, SP, QemuExecutorState<'a, QT, S>>,
    state: QemuExecutorState<'a, QT, S>,
}

#[cfg(feature = "fork")]
impl<'a, H, OT, QT, S, SP> Debug for QemuForkExecutor<'a, H, OT, QT, S, SP>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<'a, QT, S>) -> ExitKind,
    S: State + HasExecutions,
    OT: ObserversTuple<S> + Debug,
    QT: QemuHelperTuple<S> + Debug,
    SP: ShMemProvider,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("QemuForkExecutor")
            .field("hooks", &self.state.hooks)
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(feature = "fork")]
impl<'a, H, OT, QT, S, SP> QemuForkExecutor<'a, H, OT, QT, S, SP>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<'a, QT, S>) -> ExitKind,
    S: State + HasExecutions,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
    SP: ShMemProvider,
{
    pub fn new<EM, OF, Z>(
        hooks: &'a mut QemuHooks<QT, S>,
        harness_fn: &'a mut H,
        observers: OT,
        fuzzer: &mut Z,
        state: &mut S,
        event_mgr: &mut EM,
        shmem_provider: SP,
        timeout: core::time::Duration,
    ) -> Result<Self, Error>
    where
        EM: EventFirer<State = S> + EventRestarter,
        OF: Feedback<S>,
        S: HasSolutions,
        Z: HasObjective<Objective = OF, State = S>,
    {
        assert!(!QT::HOOKS_DO_SIDE_EFFECTS, "When using QemuForkExecutor, the hooks must not do any side effect as they will happen in the child process and then discarded");

        Ok(Self {
            inner: InProcessForkExecutor::new(
                harness_fn,
                observers,
                fuzzer,
                state,
                event_mgr,
                timeout,
                shmem_provider,
            )?,
            state: QemuExecutorState {
                first_exec: true,
                hooks,
            },
        })
    }

    pub fn inner(&self) -> &InProcessForkExecutor<'a, H, OT, S, SP, QemuExecutorState<'a, QT, S>> {
        &self.inner
    }

    pub fn inner_mut(
        &mut self,
    ) -> &mut InProcessForkExecutor<'a, H, OT, S, SP, QemuExecutorState<'a, QT, S>> {
        &mut self.inner
    }

    pub fn hooks(&self) -> &QemuHooks<QT, S> {
        self.state.hooks
    }

    pub fn hooks_mut(&mut self) -> &mut QemuHooks<QT, S> {
        self.state.hooks
    }

    pub fn emulator(&self) -> &Emulator {
        self.state.hooks.emulator()
    }
}

#[cfg(feature = "fork")]
impl<'a, EM, H, OT, QT, S, Z, SP> Executor<EM, Z, NopExecutorState>
    for QemuForkExecutor<'a, H, OT, QT, S, SP>
where
    EM: EventManager<
        InProcessForkExecutor<'a, H, OT, S, SP, QemuExecutorState<'a, QT, S>>,
        Z,
        State = S,
    >,
    H: FnMut(&S::Input, &mut QemuExecutorState<'a, QT, S>) -> ExitKind,
    S: State + HasMetadata + HasExecutions + HasLastReportTime,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
    SP: ShMemProvider,
    Z: UsesState<State = S>,
{
    fn run_target(
        &mut self,
        fuzzer: &mut Z,
        state: &mut Self::State,
        mgr: &mut EM,
        input: &Self::Input,
        _executor_state: &mut (),
    ) -> Result<ExitKind, Error> {
        let emu = Emulator::get().unwrap();
        if self.state.first_exec {
            self.state.hooks.helpers().first_exec_all(self.state.hooks);
            self.state.first_exec = false;
        }
        self.state.hooks.helpers_mut().pre_exec_all(&emu, input);
        let mut exit_kind = self
            .inner
            .run_target(fuzzer, state, mgr, input, &mut self.state)?;
        self.state.hooks.helpers_mut().post_exec_all(
            &emu,
            input,
            self.inner.observers_mut(),
            &mut exit_kind,
        );
        Ok(exit_kind)
    }
}

#[cfg(feature = "fork")]
impl<'a, H, OT, QT, S, SP> UsesObservers for QemuForkExecutor<'a, H, OT, QT, S, SP>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<'a, QT, S>) -> ExitKind,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
    S: State + HasExecutions,
    SP: ShMemProvider,
{
    type Observers = OT;
}

#[cfg(feature = "fork")]
impl<'a, H, OT, QT, S, SP> UsesState for QemuForkExecutor<'a, H, OT, QT, S, SP>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<'a, QT, S>) -> ExitKind,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
    S: State + HasExecutions,
    SP: ShMemProvider,
{
    type State = S;
}

#[cfg(feature = "fork")]
impl<'a, H, OT, QT, S, SP> HasObservers for QemuForkExecutor<'a, H, OT, QT, S, SP>
where
    H: FnMut(&S::Input, &mut QemuExecutorState<'a, QT, S>) -> ExitKind,
    S: State + HasExecutions,
    OT: ObserversTuple<S>,
    QT: QemuHelperTuple<S>,
    SP: ShMemProvider,
{
    #[inline]
    fn observers(&self) -> &OT {
        self.inner.observers()
    }

    #[inline]
    fn observers_mut(&mut self) -> &mut OT {
        self.inner.observers_mut()
    }
}
