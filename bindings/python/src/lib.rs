use pyo3::prelude::*;

/// Smoke-test function proving the PyO3/maturin toolchain builds end-to-end.
#[pyfunction]
fn hello() -> String {
    "z3rno bindings scaffold OK".to_string()
}

#[pymodule]
fn z3rno(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(hello, m)?)?;
    Ok(())
}
