pub(super) const CORE_SOURCE: &str = r#"
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Default)]
pub struct SharedHandle { value: AtomicU32 }
impl SharedHandle {
    pub fn increment(&self) { self.value.fetch_add(1, Ordering::SeqCst); }
    pub fn value(&self) -> u32 { self.value.load(Ordering::SeqCst) }
}
#[derive(Default)]
pub struct MutableHandle { value: Cell<u32> }
impl MutableHandle {
    pub fn increment(&mut self) { self.value.set(self.value.get() + 1); }
    pub fn value(&self) -> u32 { self.value.get() }
}
#[derive(Default)]
pub struct LocalHandle { value: Rc<u32> }
impl LocalHandle {
    pub fn value(&self) -> u32 { *self.value }
}
"#;

pub(super) const RUNTIME_TEST: &str = r#"
#[test]
fn generated_handles_are_callable_from_another_python_thread() {
    Python::initialize();
    let (shared, mutable) = Python::attach(|py| {
        let shared = SharedHandle { inner: Arc::new(test_lib::SharedHandle::default()) };
        let mutable = MutableHandle { inner: Arc::new(std::sync::Mutex::new(test_lib::MutableHandle::default())) };
        (Py::new(py, shared).unwrap(), Py::new(py, mutable).unwrap())
    });
    std::thread::spawn(move || Python::attach(|py| {
        shared.bind(py).call_method0("increment").unwrap();
        assert_eq!(shared.bind(py).call_method0("value").unwrap().extract::<u32>().unwrap(), 1);
        mutable.bind(py).call_method0("increment").unwrap();
        assert_eq!(mutable.bind(py).call_method0("value").unwrap().extract::<u32>().unwrap(), 1);
    })).join().expect("generated handles must work on the receiving thread");
}
"#;

pub(super) const CORE_MANIFEST: &str = r#"
[package]
name = "test-lib"
version = "0.1.0"
edition = "2024"
"#;

pub(super) const BINDING_MANIFEST: &str = r#"
[package]
name = "test-lib-py"
version = "0.1.0"
edition = "2024"
[dependencies]
pyo3 = "=0.29.2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
test-lib = { path = "../test-lib" }
"#;
