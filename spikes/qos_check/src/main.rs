fn main() {
    // Check libc exposes what we need on macOS.
    #[cfg(target_os = "macos")]
    unsafe {
        let mut ncpu: i32 = 0;
        let mut size = std::mem::size_of::<i32>();
        let name = std::ffi::CString::new("hw.perflevel1.physicalcpu").unwrap();
        let r = libc::sysctlbyname(name.as_ptr(), &mut ncpu as *mut _ as *mut _, &mut size, std::ptr::null_mut(), 0);
        println!("sysctlbyname result={r} efficiency_cores={ncpu}");

        let r2 = libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
        println!("pthread_set_qos_class_self_np result={r2}");
    }
}
