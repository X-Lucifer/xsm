//! Small audited boundary for clearing password buffers without violating String invariants.
use std::sync::atomic::{Ordering, compiler_fence};

pub(crate) fn wipe<T: Copy + Default>(values: &mut [T]) {
    for value in values {
        // SAFETY: `value` is an exclusive, aligned reference to one initialized T.
        // T is Copy (no destructor is bypassed) and Default creates a valid value.
        // Volatile stores prevent dead-store elimination of sensitive data clearing.
        unsafe { std::ptr::write_volatile(value, T::default()) };
    }
    compiler_fence(Ordering::SeqCst);
}

pub(crate) fn wipe_string(value: &mut String) {
    // Move out through a safe conversion rather than exposing String's UTF-8 bytes
    // mutably. The original String stays valid and empty throughout the operation.
    let mut bytes = std::mem::take(value).into_bytes();
    wipe(&mut bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clears_utf8_and_utf16_without_invalid_strings() {
        let mut text = String::from("密码🔑secret");
        wipe_string(&mut text);
        assert_eq!(text, "");
        let mut wide: Vec<u16> = "password密码".encode_utf16().collect();
        wipe(&mut wide);
        assert!(wide.iter().all(|&unit| unit == 0));
    }
}
