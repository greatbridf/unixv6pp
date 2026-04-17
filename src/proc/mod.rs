/// A channel that sleepers can subscribe to.
///
/// Used by sleep and wakeup family functions.
pub trait Channel: Sized {
    #[inline(always)]
    fn channel_addr(&self) -> usize;
}

impl<T> Channel for &T {
    #[inline(always)]
    fn channel_addr(&self) -> usize {
        *self as *const T as usize
    }
}

extern "C" {
    fn _wakeup_all(channel_addr: usize);
    fn _sleep(channel_addr: usize);
}

pub fn wakeup_all(channel: impl Channel) {
    unsafe {
        _wakeup_all(channel.channel_addr());
    }
}

pub fn sleep(channel: impl Channel) {
    unsafe {
        _sleep(channel.channel_addr());
    }
}
