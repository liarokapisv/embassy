use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::{Condvar, Mutex, Once};
use std::time::{Duration as StdDuration, Instant as StdInstant};
use std::{mem, ptr, thread};

use atomic_polyfill::{AtomicU8, Ordering};

use crate::driver::{AlarmHandle, Driver};

const ALARM_COUNT: usize = 4;

struct AlarmState {
    timestamp: u64,

    // This is really a Option<(fn(*mut ()), *mut ())>
    // but fn pointers aren't allowed in const yet
    callback: *const (),
    ctx: *mut (),
}

unsafe impl Send for AlarmState {}

impl AlarmState {
    const fn new() -> Self {
        Self {
            timestamp: u64::MAX,
            callback: ptr::null(),
            ctx: ptr::null_mut(),
        }
    }
}

struct TimeDriver {
    alarm_count: AtomicU8,

    once: Once,
    alarms: UninitCell<Mutex<[AlarmState; ALARM_COUNT]>>,
    zero_instant: UninitCell<StdInstant>,
    signaler: UninitCell<Signaler>,
}

const ALARM_NEW: AlarmState = AlarmState::new();
crate::time_driver_impl!(static DRIVER: TimeDriver = TimeDriver {
    alarm_count: AtomicU8::new(0),

    once: Once::new(),
    alarms: UninitCell::uninit(),
    zero_instant: UninitCell::uninit(),
    signaler: UninitCell::uninit(),
});

impl TimeDriver {
    fn init(&self) {
        self.once.call_once(|| unsafe {
            self.alarms.write(Mutex::new([ALARM_NEW; ALARM_COUNT]));
            self.zero_instant.write(StdInstant::now());
            self.signaler.write(Signaler::new());

            thread::spawn(Self::alarm_thread);
        });
    }

    fn alarm_thread() {
        let zero = unsafe { DRIVER.zero_instant.read() };
        loop {
            let now = DRIVER.now();

            let mut next_alarm = u64::MAX;
            {
                let alarms = &mut *unsafe { DRIVER.alarms.as_ref() }.lock().unwrap();
                for alarm in alarms {
                    if alarm.timestamp <= now {
                        alarm.timestamp = u64::MAX;

                        // Call after clearing alarm, so the callback can set another alarm.

                        // safety:
                        // - we can ignore the possiblity of `f` being unset (null) because of the safety contract of `allocate_alarm`.
                        // - other than that we only store valid function pointers into alarm.callback
                        let f: fn(*mut ()) = unsafe { mem::transmute(alarm.callback) };
                        f(alarm.ctx);
                    } else {
                        next_alarm = next_alarm.min(alarm.timestamp);
                    }
                }
            }

            // Ensure we don't overflow
            let until = zero
                .checked_add(StdDuration::from_micros(next_alarm))
                .unwrap_or_else(|| StdInstant::now() + StdDuration::from_secs(1));

            unsafe { DRIVER.signaler.as_ref() }.wait_until(until);
        }
    }
}

impl Driver for TimeDriver {
    fn now(&self) -> u64 {
        self.init();

        let zero = unsafe { self.zero_instant.read() };
        StdInstant::now().duration_since(zero).as_micros() as u64
    }

    unsafe fn allocate_alarm(&self) -> Option<AlarmHandle> {
        let id = self.alarm_count.fetch_update(Ordering::AcqRel, Ordering::Acquire, |x| {
            if x < ALARM_COUNT as u8 {
                Some(x + 1)
            } else {
                None
            }
        });

        match id {
            Ok(id) => Some(AlarmHandle::new(id)),
            Err(_) => None,
        }
    }

    fn set_alarm_callback(&self, alarm: AlarmHandle, callback: fn(*mut ()), ctx: *mut ()) {
        self.init();
        let mut alarms = unsafe { self.alarms.as_ref() }.lock().unwrap();
        let alarm = &mut alarms[alarm.id() as usize];
        alarm.callback = callback as *const ();
        alarm.ctx = ctx;
    }

    fn set_alarm(&self, alarm: AlarmHandle, timestamp: u64) -> bool {
        self.init();
        let mut alarms = unsafe { self.alarms.as_ref() }.lock().unwrap();
        let alarm = &mut alarms[alarm.id() as usize];
        alarm.timestamp = timestamp;
        unsafe { self.signaler.as_ref() }.signal();

        true
    }
}

struct Signaler {
    mutex: Mutex<bool>,
    condvar: Condvar,
}

impl Signaler {
    fn new() -> Self {
        Self {
            mutex: Mutex::new(false),
            condvar: Condvar::new(),
        }
    }

    fn wait_until(&self, until: StdInstant) {
        let mut signaled = self.mutex.lock().unwrap();
        while !*signaled {
            let now = StdInstant::now();

            if now >= until {
                break;
            }

            let dur = until - now;
            let (signaled2, timeout) = self.condvar.wait_timeout(signaled, dur).unwrap();
            signaled = signaled2;
            if timeout.timed_out() {
                break;
            }
        }
        *signaled = false;
    }

    fn signal(&self) {
        let mut signaled = self.mutex.lock().unwrap();
        *signaled = true;
        self.condvar.notify_one();
    }
}

pub(crate) struct UninitCell<T>(MaybeUninit<UnsafeCell<T>>);
unsafe impl<T> Send for UninitCell<T> {}
unsafe impl<T> Sync for UninitCell<T> {}

impl<T> UninitCell<T> {
    pub const fn uninit() -> Self {
        Self(MaybeUninit::uninit())
    }

    pub unsafe fn as_ptr(&self) -> *const T {
        (*self.0.as_ptr()).get()
    }

    pub unsafe fn as_mut_ptr(&self) -> *mut T {
        (*self.0.as_ptr()).get()
    }

    pub unsafe fn as_ref(&self) -> &T {
        &*self.as_ptr()
    }

    pub unsafe fn write(&self, val: T) {
        ptr::write(self.as_mut_ptr(), val)
    }
}

impl<T: Copy> UninitCell<T> {
    pub unsafe fn read(&self) -> T {
        ptr::read(self.as_mut_ptr())
    }
}
