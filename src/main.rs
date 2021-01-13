#![feature(linked_list_cursors)]

use std::sync::{Arc, Mutex};
use std::sync::mpsc::channel;
use std::sync::mpsc::{Sender, Receiver};
use std::collections::LinkedList;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::boxed::Box;

type DelayedWorkResult = Result<(), &'static str>;

type Work = Box<dyn FnMut() + Send + 'static>;

type ProtectedList<T> = Arc<Mutex<LinkedList<T>>>;

struct Timeout {
    work: Work,
    ticks: u128,
    dbg_init_ticks: u128,
    dbg_expected_trigger: u128,
}

impl Timeout {
    fn new(work: Work, delay_ms: u128) -> Timeout {
        let current_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis();

        Timeout {
            work,
            ticks: delay_ms,
            dbg_init_ticks: delay_ms,
            dbg_expected_trigger: current_millis + delay_ms,
        }
    }
}

impl std::fmt::Debug for Timeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Timeout")
            .field("ticks", &self.ticks)
            .field("initial_ticks", &self.dbg_init_ticks)
            .field("expected_trigger", &self.dbg_expected_trigger)
            .finish()
    }
}

fn work_a(i: u64) {
    let start = SystemTime::now();
    let current_millis = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis();

    println!("a {}: {}", i, current_millis);
}

fn work_b(msg: String) {
    let start = SystemTime::now();
    let current_millis = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis();

    println!("b: {}", msg);
    println!("b: {}", current_millis);
}

fn timeout_thread(work_sender: Sender<Work>, list_protected: ProtectedList<Timeout>) {
    // TODO: Ideally this should be implemented with an
    // interrupt timer instead we just sleep some and keep
    // track of the passed milliseconds to an acceptable
    // accuracy (for this test).
    // This is very naive, just wanted to test the scheduler
    // logic.
    let mut delta_micros = 0;
    let mut last_micro = SystemTime::now();
    loop {

        thread::sleep(Duration::from_micros(250));

        let micros_slept = last_micro.elapsed()
            .expect("Failed to get elapsed time")
            .as_micros();

        delta_micros += micros_slept;

        last_micro = SystemTime::now();

        if delta_micros < 1000 {
            continue;
        }
        delta_micros -= 1000;

        let mut list = list_protected.lock()
            .expect("Failed to lock timeout queue");
        let mut t = match list.front_mut() {
            Some(item) => item,
            None => continue,
        };

        if let None = t.ticks.checked_sub(1) {
            let expired = list.pop_front().unwrap();
            work_sender.send(expired.work)
                .expect("Failed to send delayed work");

            let current_millis = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_millis();
            println!("Expected expiration {}", expired.dbg_expected_trigger);
            println!("Actual expiration {}", current_millis);
        } else {
            t.ticks -= 1;
        }
    }
}

fn worker_thread(work_receiver: Receiver<Work>) {
    loop {
        match work_receiver.recv() {
            Ok(mut work) => {
                println!("Executing work,");
                work();
            },
            Err(e) => {
                println!("{:?}", e);
                break
            }
        }
    }
}

fn delayed_work_submit(work: Work, delay_ms: u128, list_protected: &mut ProtectedList<Timeout>) -> DelayedWorkResult {
    let mut list = list_protected.lock().or_else(|_| Err("Lock failed"))?;
    let mut new = Timeout::new(work, delay_ms);
    let mut list_cursor = list.cursor_front_mut();

    while let Some(t) = list_cursor.current() {

        if t.ticks > new.ticks {
            t.ticks -= new.ticks;
            list_cursor.insert_before(new);
            return Ok(());
        }

        new.ticks -= t.ticks;

        list_cursor.move_next();
    }

    list_cursor.insert_after(new);

    Ok(())
}

fn main() {
    let timeouts: LinkedList<Timeout> = LinkedList::new();
    let (work_sender, work_receiver) = channel();
    let timeout_work_sender = work_sender.clone();

    let mut list_lock = Arc::new(Mutex::new(timeouts));
    let timeout_list_lock = list_lock.clone();

    let worker_thread = thread::spawn(|| worker_thread(work_receiver));
    thread::spawn(|| timeout_thread(timeout_work_sender, timeout_list_lock));

    thread::sleep(Duration::from_millis(100));

    work_sender.send(Box::new(|| {
        work_a(64)
    })).unwrap();

    work_sender.send(Box::new(|| {
        work_b("From main".to_string())
    })).unwrap();

    delayed_work_submit(Box::new(|| {
        work_b("Hello, 200ms later!".to_string());
    }), 200, &mut list_lock).unwrap();
    delayed_work_submit(Box::new(|| {
        work_b("Hello, 50ms later!".to_string());
    }), 50, &mut list_lock).unwrap();
    delayed_work_submit(Box::new(|| {
        work_b("Hello, 100ms later!".to_string());
    }), 100, &mut list_lock).unwrap();

    worker_thread.join().unwrap()
}
