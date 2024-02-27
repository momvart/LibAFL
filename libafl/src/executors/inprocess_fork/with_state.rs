//! The `GenericInProcessForkExecutorWithState` to do forking before executing the harness in-processly. Harness can access internal state.
use core::{
    fmt::{self, Debug, Formatter},
    marker::PhantomData,
    time::Duration,
};

use libafl_bolts::{shmem::ShMemProvider, tuples::tuple_list};
use nix::unistd::{fork, ForkResult};

use super::super::hooks::ExecutorHooksTuple;
#[cfg(all(unix, not(target_os = "linux")))]
use crate::executors::hooks::timer::{setitimer, Itimerval, Timeval, ITIMER_REAL};
use crate::{
    events::{EventFirer, EventRestarter},
    executors::{
        inprocess_fork::GenericInProcessForkExecutorInner, Executor, ExitKind, HasExecutorState,
        HasObservers,
    },
    feedbacks::Feedback,
    fuzzer::HasObjective,
    inputs::UsesInput,
    observers::{ObserversTuple, UsesObservers},
    state::{HasExecutions, HasSolutions, State, UsesState},
    Error,
};

/// The `InProcessForkExecutorWithState` with no user hooks
pub type InProcessForkExecutorWithState<'a, H, OT, S, SP, ES, EM, Z> =
    GenericInProcessForkExecutorWithState<'a, H, (), OT, S, SP, ES, EM, Z>;

impl<'a, H, OT, S, SP, ES, EM, Z, OF> InProcessForkExecutorWithState<'a, H, OT, S, SP, ES, EM, Z>
where
    H: FnMut(&S::Input, &mut ES::ExecutorState) -> ExitKind + ?Sized,
    OT: ObserversTuple<S>,
    SP: ShMemProvider,
    ES: HasExecutorState,
    EM: EventFirer<State = S> + EventRestarter<State = S>,
    OF: Feedback<S>,
    S: State + HasSolutions,
    Z: HasObjective<Objective = OF, State = S>,
{
    #[allow(clippy::too_many_arguments)]
    /// The constructor for `InProcessForkExecutor`
    pub fn new(
        harness_fn: &'a mut H,
        observers: OT,
        fuzzer: &mut Z,
        state: &mut S,
        event_mgr: &mut EM,
        timeout: Duration,
        shmem_provider: SP,
    ) -> Result<Self, Error> {
        Self::with_hooks(
            tuple_list!(),
            harness_fn,
            observers,
            fuzzer,
            state,
            event_mgr,
            timeout,
            shmem_provider,
        )
    }
}

/// [`GenericInProcessForkExecutorWithState`] is an executor that forks the current process before each execution. Harness can access some internal state.
pub struct GenericInProcessForkExecutorWithState<'a, H, HT, OT, S, SP, ES, EM, Z>
where
    H: FnMut(&S::Input, &mut ES::ExecutorState) -> ExitKind + ?Sized,
    OT: ObserversTuple<S>,
    S: UsesInput,
    SP: ShMemProvider,
    HT: ExecutorHooksTuple,
    ES: HasExecutorState,
    EM: UsesState<State = S>,
    Z: UsesState<State = S>,
{
    harness_fn: &'a mut H,
    inner: GenericInProcessForkExecutorInner<HT, OT, S, SP, EM, Z>,
    phantom: PhantomData<ES>,
}

impl<'a, H, HT, OT, S, SP, ES, EM, Z> Debug
    for GenericInProcessForkExecutorWithState<'a, H, HT, OT, S, SP, ES, EM, Z>
where
    H: FnMut(&S::Input, &mut ES::ExecutorState) -> ExitKind + ?Sized,
    OT: ObserversTuple<S> + Debug,
    S: UsesInput,
    SP: ShMemProvider,
    HT: ExecutorHooksTuple,
    ES: HasExecutorState,
    EM: UsesState<State = S>,
    Z: UsesState<State = S>,
{
    #[cfg(target_os = "linux")]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenericInProcessForkExecutor")
            .field("GenericInProcessForkExecutionInner", &self.inner)
            .finish()
    }

    #[cfg(not(target_os = "linux"))]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        #[cfg(not(target_os = "linux"))]
        return f
            .debug_struct("GenericInProcessForkExecutor")
            .field("GenericInProcessForkExecutionInner", &self.inner)
            .finish();
    }
}

impl<'a, H, HT, OT, S, SP, ES, EM, Z> UsesState
    for GenericInProcessForkExecutorWithState<'a, H, HT, OT, S, SP, ES, EM, Z>
where
    H: FnMut(&S::Input, &mut ES::ExecutorState) -> ExitKind + ?Sized,
    OT: ObserversTuple<S>,
    S: State,
    SP: ShMemProvider,
    HT: ExecutorHooksTuple,
    ES: HasExecutorState,
    EM: UsesState<State = S>,
    Z: UsesState<State = S>,
{
    type State = S;
}

impl<'a, EM, H, HT, OT, S, SP, Z, ES, OF> Executor<EM, Z, ES>
    for GenericInProcessForkExecutorWithState<'a, H, HT, OT, S, SP, ES, EM, Z>
where
    H: FnMut(&S::Input, &mut ES::ExecutorState) -> ExitKind + ?Sized,
    OT: ObserversTuple<S> + Debug,
    S: State + HasExecutions,
    SP: ShMemProvider,
    HT: ExecutorHooksTuple,
    ES: HasExecutorState,
    EM: EventFirer<State = S> + EventRestarter<State = S>,
    Z: HasObjective<Objective = OF, State = S>,
    OF: Feedback<S>,
{
    #[allow(unreachable_code)]
    #[inline]
    fn run_target(
        &mut self,
        fuzzer: &mut Z,
        state: &mut Self::State,
        mgr: &mut EM,
        input: &Self::Input,
        execution_state: &mut ES::ExecutorState,
    ) -> Result<ExitKind, Error> {
        *state.executions_mut() += 1;

        unsafe {
            self.inner.shmem_provider.pre_fork()?;
            match fork() {
                Ok(ForkResult::Child) => {
                    // Child
                    self.inner.pre_run_target_child(fuzzer, state, mgr, input)?;
                    (self.harness_fn)(input, execution_state);
                    self.inner.post_run_target_child(fuzzer, state, mgr, input);
                    Ok(ExitKind::Ok)
                }
                Ok(ForkResult::Parent { child }) => {
                    // Parent
                    self.inner.parent(child)
                }
                Err(e) => Err(Error::from(e)),
            }
        }
    }
}

impl<'a, H, HT, OT, S, SP, ES, EM, Z, OF>
    GenericInProcessForkExecutorWithState<'a, H, HT, OT, S, SP, ES, EM, Z>
where
    H: FnMut(&S::Input, &mut ES::ExecutorState) -> ExitKind + ?Sized,
    HT: ExecutorHooksTuple,
    OT: ObserversTuple<S>,
    SP: ShMemProvider,
    ES: HasExecutorState,
    Z: UsesState<State = S>,
    EM: EventFirer<State = S> + EventRestarter<State = S>,
    OF: Feedback<S>,
    S: State + HasSolutions,
    Z: HasObjective<Objective = OF, State = S>,
{
    /// Creates a new [`GenericInProcessForkExecutorWithState`] with custom hooks
    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_arguments)]
    pub fn with_hooks(
        userhooks: HT,
        harness_fn: &'a mut H,
        observers: OT,
        fuzzer: &mut Z,
        state: &mut S,
        event_mgr: &mut EM,
        timeout: Duration,
        shmem_provider: SP,
    ) -> Result<Self, Error> {
        Ok(Self {
            harness_fn,
            inner: GenericInProcessForkExecutorInner::with_hooks(
                userhooks,
                observers,
                fuzzer,
                state,
                event_mgr,
                timeout,
                shmem_provider,
            )?,
            phantom: PhantomData,
        })
    }

    /// Creates a new [`GenericInProcessForkExecutor`], non linux
    #[cfg(not(target_os = "linux"))]
    #[allow(clippy::too_many_arguments)]
    pub fn with_hooks<EM, OF, Z>(
        userhooks: HT,
        harness_fn: &'a mut H,
        observers: OT,
        _fuzzer: &mut Z,
        state: &mut S,
        _event_mgr: &mut EM,
        timeout: Duration,
        shmem_provider: SP,
    ) -> Result<Self, Error>
    where
        EM: EventFirer<State = S> + EventRestarter<State = S>,
        OF: Feedback<S>,
        S: HasSolutions,
        Z: HasObjective<Objective = OF, State = S>,
    {
        Ok(Self {
            harness_fn,
            inner: GenericInProcessForkExecutorInner::with_hooks(
                userhooks,
                observers,
                fuzzer,
                state,
                event_mgr,
                timeout,
                shmem_provider,
            )?,
            phantom: PhantomData,
        })
    }

    /// Retrieve the harness function.
    #[inline]
    pub fn harness(&self) -> &H {
        self.harness_fn
    }

    /// Retrieve the harness function for a mutable reference.
    #[inline]
    pub fn harness_mut(&mut self) -> &mut H {
        self.harness_fn
    }
}

impl<'a, H, HT, OT, S, SP, ES, EM, Z> UsesObservers
    for GenericInProcessForkExecutorWithState<'a, H, HT, OT, S, SP, ES, EM, Z>
where
    H: FnMut(&S::Input, &mut ES::ExecutorState) -> ExitKind + ?Sized,
    HT: ExecutorHooksTuple,
    OT: ObserversTuple<S>,
    S: State,
    SP: ShMemProvider,
    ES: HasExecutorState,
    EM: UsesState<State = S>,
    Z: UsesState<State = S>,
{
    type Observers = OT;
}

impl<'a, H, HT, OT, S, SP, ES, EM, Z> HasObservers
    for GenericInProcessForkExecutorWithState<'a, H, HT, OT, S, SP, ES, EM, Z>
where
    H: FnMut(&S::Input, &mut ES::ExecutorState) -> ExitKind + ?Sized,
    HT: ExecutorHooksTuple,
    S: State,
    OT: ObserversTuple<S>,
    SP: ShMemProvider,
    ES: HasExecutorState,
    EM: UsesState<State = S>,
    Z: UsesState<State = S>,
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

#[cfg(test)]
mod tests {
    use libafl_bolts::tuples::tuple_list;

    use crate::{executors::ExitKind, inputs::NopInput};

    #[test]
    #[cfg_attr(miri, ignore)]
    #[cfg(all(feature = "std", feature = "fork", unix))]
    fn test_inprocessfork_exec() {
        use core::marker::PhantomData;

        use libafl_bolts::shmem::{ShMemProvider, StdShMemProvider};
        #[cfg(target_os = "linux")]
        use libc::{itimerspec, timespec};

        #[cfg(not(target_os = "linux"))]
        use crate::executors::hooks::timer::{Itimerval, Timeval};
        use crate::{
            events::SimpleEventManager,
            executors::{
                hooks::inprocess_fork::InChildProcessHooks,
                inprocess_fork::GenericInProcessForkExecutor, Executor,
            },
            fuzzer::test::NopFuzzer,
            state::test::NopState,
        };

        let provider = StdShMemProvider::new().unwrap();

        #[cfg(target_os = "linux")]
        let timespec = timespec {
            tv_sec: 5,
            tv_nsec: 0,
        };
        #[cfg(target_os = "linux")]
        let itimerspec = itimerspec {
            it_interval: timespec,
            it_value: timespec,
        };

        #[cfg(not(target_os = "linux"))]
        let timespec = Timeval {
            tv_sec: 5,
            tv_usec: 0,
        };
        #[cfg(not(target_os = "linux"))]
        let itimerspec = Itimerval {
            it_interval: timespec,
            it_value: timespec,
        };

        let mut harness = |_buf: &NopInput| ExitKind::Ok;
        let default = InChildProcessHooks::nop();
        #[cfg(target_os = "linux")]
        let mut in_process_fork_executor = GenericInProcessForkExecutorWithState::<_, (), (), _, _> {
            hooks: tuple_list!(default),
            harness_fn: &mut harness,
            shmem_provider: provider,
            observers: tuple_list!(),
            itimerspec,
            phantom: PhantomData,
        };
        #[cfg(not(target_os = "linux"))]
        let mut in_process_fork_executor = GenericInProcessForkExecutor::<_, (), (), _, _> {
            harness_fn: &mut harness,
            shmem_provider: provider,
            observers: tuple_list!(),
            hooks: tuple_list!(default),
            itimerval: itimerspec,
            phantom: PhantomData,
        };
        let input = NopInput {};
        let mut fuzzer = NopFuzzer::new();
        let mut state = NopState::new();
        let mut mgr = SimpleEventManager::printing();
        in_process_fork_executor
            .run_target(&mut fuzzer, &mut state, &mut mgr, &input)
            .unwrap();
    }
}
