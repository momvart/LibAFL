//! Executors take input, and run it in the target.

pub mod inprocess;
pub use inprocess::InProcessExecutor;
#[cfg(all(feature = "std", feature = "fork", unix))]
pub use inprocess::InProcessForkExecutor;

pub mod differential;
pub use differential::DiffExecutor;

/// Timeout executor.
/// Not possible on `no-std` Windows or `no-std`, but works for unix
#[cfg(any(unix, feature = "std"))]
pub mod timeout;
#[cfg(any(unix, feature = "std"))]
pub use timeout::TimeoutExecutor;

#[cfg(all(feature = "std", feature = "fork", unix))]
pub mod forkserver;
#[cfg(all(feature = "std", feature = "fork", unix))]
pub use forkserver::{Forkserver, ForkserverExecutor, TimeoutForkserverExecutor};

pub mod combined;
pub use combined::CombinedExecutor;

pub mod shadow;
pub use shadow::ShadowExecutor;

pub mod with_observers;
pub use with_observers::WithObservers;

#[cfg(all(feature = "std", any(unix, doc)))]
pub mod command;
use core::{fmt::Debug, marker::PhantomData};

#[cfg(all(feature = "std", any(unix, doc)))]
pub use command::CommandExecutor;
use serde::{Deserialize, Serialize};

use crate::{
    bolts::AsSlice,
    inputs::{HasTargetBytes, Input},
    observers::ObserversTuple,
    prelude::State,
    Error,
};

/// How an execution finished.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExitKind {
    /// The run exited normally.
    Ok,
    /// The run resulted in a target crash.
    Crash,
    /// The run hit an out of memory error.
    Oom,
    /// The run timed out
    Timeout,
    /// Special case for [`DiffExecutor`] when both exitkinds don't match
    Diff {
        /// The exitkind of the primary executor
        primary: DiffExitKind,
        /// The exitkind of the secondary executor
        secondary: DiffExitKind,
    },
    // The run resulted in a custom `ExitKind`.
    // Custom(Box<dyn SerdeAny>),
}

/// How one of the diffing executions finished.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DiffExitKind {
    /// The run exited normally.
    Ok,
    /// The run resulted in a target crash.
    Crash,
    /// The run hit an out of memory error.
    Oom,
    /// The run timed out
    Timeout,
    /// One of the executors itelf repots a differential, we can't go into further details.
    Diff,
    // The run resulted in a custom `ExitKind`.
    // Custom(Box<dyn SerdeAny>),
}

crate::impl_serdeany!(ExitKind);

impl From<ExitKind> for DiffExitKind {
    fn from(exitkind: ExitKind) -> Self {
        match exitkind {
            ExitKind::Ok => DiffExitKind::Ok,
            ExitKind::Crash => DiffExitKind::Crash,
            ExitKind::Oom => DiffExitKind::Oom,
            ExitKind::Timeout => DiffExitKind::Timeout,
            ExitKind::Diff { .. } => DiffExitKind::Diff,
        }
    }
}

crate::impl_serdeany!(DiffExitKind);

/// Holds a tuple of Observers
pub trait HasObservers: Debug {
    /// The [`Input`]
    type Input: Input;
    /// The [`State`]
    type State: State<Input = Self::Input>;
    /// The observers type
    type Observers: ObserversTuple;

    /// Get the linked observers
    fn observers(&self) -> &Self::Observers;

    /// Get the linked observers (mutable)
    fn observers_mut(&mut self) -> &mut Self::Observers;
}

/// An executor takes the given inputs, and runs the harness/target.
pub trait Executor<EM, I, S, Z>: Debug
where
    I: Input,
    Z: Sized,
{
    /// Instruct the target about the input and run
    fn run_target(
        &mut self,
        fuzzer: &mut Z,
        state: &mut S,
        mgr: &mut EM,
        input: &I,
    ) -> Result<ExitKind, Error>;

    /// Wraps this Executor with the given [`ObserversTuple`] to implement [`HasObservers`].
    ///
    /// If the executor already implements [`HasObservers`], then the original implementation will be overshadowed by
    /// the implementation of this wrapper.
    fn with_observers<OT>(self, observers: OT) -> WithObservers<Self, OT>
    where
        Self: Sized,
        OT: ObserversTuple<Input = I, State = S>,
    {
        WithObservers::new(self, observers)
    }

    /// Custom Reset Handler, e.g., to reset timers
    #[inline]
    fn post_run_reset(&mut self) {}
}

/// A simple executor that does nothing.
/// If intput len is 0, `run_target` will return Err
#[derive(Debug)]
struct NopExecutor<I, S> {
    phantom: PhantomData<(I, S)>,
}

impl<EM, I, S, Z> Executor<EM, I, S, Z> for NopExecutor<I, S>
where
    I: Input + HasTargetBytes,
    S: State<Input = I> + Debug,
{
    fn run_target(
        &mut self,
        _fuzzer: &mut Z,
        _state: &mut S,
        _mgr: &mut EM,
        input: &I,
    ) -> Result<ExitKind, Error> {
        if input.target_bytes().as_slice().is_empty() {
            Err(Error::empty("Input Empty"))
        } else {
            Ok(ExitKind::Ok)
        }
    }
}

#[cfg(test)]
mod test {
    use core::marker::PhantomData;

    use super::{Executor, NopExecutor};
    use crate::inputs::BytesInput;

    #[test]
    fn nop_executor() {
        let empty_input = BytesInput::new(vec![]);
        let nonempty_input = BytesInput::new(vec![1u8]);
        let mut executor = NopExecutor {
            phantom: PhantomData,
        };
        executor
            .run_target(&mut (), &mut (), &mut (), &empty_input)
            .unwrap_err();
        executor
            .run_target(&mut (), &mut (), &mut (), &nonempty_input)
            .unwrap();
    }
}

#[cfg(feature = "python")]
#[allow(missing_docs)]
/// `Executor` Python bindings
pub mod pybind {
    use pyo3::prelude::*;
    use serde::{Deserialize, Serialize};

    use crate::{
        events::pybind::PythonEventManager,
        executors::{
            inprocess::pybind::PythonOwnedInProcessExecutor, Executor, ExitKind, HasObservers,
        },
        fuzzer::pybind::{PythonStdFuzzer, PythonStdFuzzerWrapper},
        inputs::{BytesInput, HasBytesVec},
        observers::pybind::PythonObserversTuple,
        state::pybind::{PythonStdState, PythonStdStateWrapper},
        Error,
    };

    #[pyclass(unsendable, name = "ExitKind")]
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct PythonExitKind {
        pub inner: ExitKind,
    }

    impl From<ExitKind> for PythonExitKind {
        fn from(inner: ExitKind) -> Self {
            Self { inner }
        }
    }

    #[pymethods]
    impl PythonExitKind {
        fn __eq__(&self, other: &PythonExitKind) -> bool {
            self.inner == other.inner
        }

        #[must_use]
        fn is_ok(&self) -> bool {
            self.inner == ExitKind::Ok
        }

        #[must_use]
        fn is_crash(&self) -> bool {
            self.inner == ExitKind::Crash
        }

        #[must_use]
        fn is_oom(&self) -> bool {
            self.inner == ExitKind::Oom
        }

        #[must_use]
        fn is_timeout(&self) -> bool {
            self.inner == ExitKind::Timeout
        }

        #[staticmethod]
        #[must_use]
        fn ok() -> Self {
            Self {
                inner: ExitKind::Ok,
            }
        }

        #[staticmethod]
        #[must_use]
        fn crash() -> Self {
            Self {
                inner: ExitKind::Crash,
            }
        }

        #[staticmethod]
        #[must_use]
        fn oom() -> Self {
            Self {
                inner: ExitKind::Oom,
            }
        }

        #[staticmethod]
        #[must_use]
        fn timeout() -> Self {
            Self {
                inner: ExitKind::Timeout,
            }
        }
    }

    #[derive(Clone, Debug)]
    pub struct PyObjectExecutor {
        inner: PyObject,
        tuple: PythonObserversTuple,
    }

    impl PyObjectExecutor {
        #[must_use]
        pub fn new(obj: PyObject) -> Self {
            let tuple = Python::with_gil(|py| -> PyResult<PythonObserversTuple> {
                obj.call_method1(py, "observers", ())?.extract(py)
            })
            .unwrap();
            PyObjectExecutor { inner: obj, tuple }
        }
    }

    impl HasObservers for PyObjectExecutor {
        type Observers = PythonObserversTuple;
        type Input = BytesInput;
        type State = PythonStdState;

        #[inline]
        fn observers(&self) -> &PythonObserversTuple {
            &self.tuple
        }

        #[inline]
        fn observers_mut(&mut self) -> &mut PythonObserversTuple {
            &mut self.tuple
        }
    }

    impl Executor<PythonEventManager, BytesInput, PythonStdState, PythonStdFuzzer>
        for PyObjectExecutor
    {
        #[inline]
        fn run_target<EM, I, S, Z>(
            &mut self,
            fuzzer: &mut PythonStdFuzzer,
            state: &mut PythonStdState,
            mgr: &mut PythonEventManager,
            input: &BytesInput,
        ) -> Result<ExitKind, Error> {
            let ek = Python::with_gil(|py| -> PyResult<_> {
                let ek: PythonExitKind = self
                    .inner
                    .call_method1(
                        py,
                        "run_target",
                        (
                            PythonStdFuzzerWrapper::wrap(fuzzer),
                            PythonStdStateWrapper::wrap(state),
                            mgr.clone(),
                            input.bytes(),
                        ),
                    )?
                    .extract(py)?;
                Ok(ek)
            })?;
            Ok(ek.inner)
        }
    }

    #[derive(Clone, Debug)]
    enum PythonExecutorWrapper {
        InProcess(Py<PythonOwnedInProcessExecutor>),
        Python(PyObjectExecutor),
    }

    #[pyclass(unsendable, name = "Executor")]
    #[derive(Clone, Debug)]
    /// Executor<Input = I> + HasObservers Trait binding
    pub struct PythonExecutor {
        wrapper: PythonExecutorWrapper,
    }

    macro_rules! unwrap_me {
        ($wrapper:expr, $name:ident, $body:block) => {
            crate::unwrap_me_body!($wrapper, $name, $body, PythonExecutorWrapper,
                { InProcess },
                {
                    Python(py_wrapper) => {
                        let $name = py_wrapper;
                        $body
                    }
                }
            )
        };
    }

    macro_rules! unwrap_me_mut {
        ($wrapper:expr, $name:ident, $body:block) => {
            crate::unwrap_me_mut_body!($wrapper, $name, $body, PythonExecutorWrapper,
                { InProcess },
                {
                    Python(py_wrapper) => {
                        let $name = py_wrapper;
                        $body
                    }
                }
            )
        };
    }

    #[pymethods]
    impl PythonExecutor {
        #[staticmethod]
        #[must_use]
        pub fn new_inprocess(owned_inprocess_executor: Py<PythonOwnedInProcessExecutor>) -> Self {
            Self {
                wrapper: PythonExecutorWrapper::InProcess(owned_inprocess_executor),
            }
        }

        #[staticmethod]
        #[must_use]
        pub fn new_py(obj: PyObject) -> Self {
            Self {
                wrapper: PythonExecutorWrapper::Python(PyObjectExecutor::new(obj)),
            }
        }

        #[must_use]
        pub fn unwrap_py(&self) -> Option<PyObject> {
            match &self.wrapper {
                PythonExecutorWrapper::Python(pyo) => Some(pyo.inner.clone()),
                PythonExecutorWrapper::InProcess(_) => None,
            }
        }
    }

    impl HasObservers {
        type Input = BytesInput;
        type Observers = PythonObserversTuple;
        type State = PythonStdState;

        #[inline]
        fn observers(&self) -> &PythonObserversTuple {
            let ptr = unwrap_me!(self.wrapper, e, {
                e.observers() as *const PythonObserversTuple
            });
            unsafe { ptr.as_ref().unwrap() }
        }

        #[inline]
        fn observers_mut(&mut self) -> &mut PythonObserversTuple {
            let ptr = unwrap_me_mut!(self.wrapper, e, {
                e.observers_mut() as *mut PythonObserversTuple
            });
            unsafe { ptr.as_mut().unwrap() }
        }
    }

    impl Executor<PythonEventManager, BytesInput, PythonStdState, PythonStdFuzzer> for PythonExecutor {
        #[inline]
        fn run_target<EM, I, S, Z>(
            &mut self,
            fuzzer: &mut PythonStdFuzzer,
            state: &mut PythonStdState,
            mgr: &mut PythonEventManager,
            input: &BytesInput,
        ) -> Result<ExitKind, Error> {
            unwrap_me_mut!(self.wrapper, e, { e.run_target(fuzzer, state, mgr, input) })
        }
    }

    /// Register the classes to the python module
    pub fn register(_py: Python, m: &PyModule) -> PyResult<()> {
        m.add_class::<PythonExitKind>()?;
        m.add_class::<PythonExecutor>()?;
        Ok(())
    }
}
