use std::ffi::{c_char, CStr, CString};

#[unsafe(no_mangle)]
pub extern "C" fn wolf_add(a: i32, b: i32) -> i32 {
    a + b
}

#[unsafe(no_mangle)]
pub extern "C" fn wolf_greet(name: *const c_char) -> *mut c_char {
    let c_str = unsafe {
        assert!(!name.is_null());
        CStr::from_ptr(name)
    };
    let r_str = c_str.to_str().unwrap();
    let greeting = format!("Hello, {} from Rust!", r_str);
    CString::new(greeting).unwrap().into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn wolf_free_string(s: *mut c_char) {
    unsafe {
        if s.is_null() { return }
        let _ = CString::from_raw(s);
    }
}
