#![feature(linked_list_cursors)]
#![feature(linked_list_remove)]
#![feature(duration_zero)]
#![feature(duration_constants)]

use std::collections::LinkedList;
use std::sync::mpsc::channel;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type Work = Box<dyn FnMut() + Send + 'static>;

struct Timeout {
    work: Work,
    delay: Duration,
    dbg_init_ticks: Duration,
    dbg_expected_trigger: Duration,
}

impl Timeout {
    fn new(work: Work, delay: Duration) -> Timeout {
        let current_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");

        Timeout {
            work,
            delay,
            dbg_init_ticks: delay,
            dbg_expected_trigger: current_millis + delay,
        }
    }
}

impl std::fmt::Debug for Timeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Timeout")
            .field("ticks", &self.delay)
            .field("initial_ticks", &self.dbg_init_ticks)
            .field("expected_trigger", &self.dbg_expected_trigger)
            .finish()
    }
}

fn timeouts_add_timeout(list: &mut LinkedList<Timeout>, mut new: Timeout) {
    let mut list_cursor = list.cursor_front_mut();

    while let Some(t) = list_cursor.current() {
        if t.delay > new.delay {
            t.delay -= new.delay;
            list_cursor.insert_before(new);
            return;
        }

        new.delay -= t.delay;

        list_cursor.move_next();
    }

    list_cursor.insert_after(new);
}

fn timekeeper_thread(work_sender: Sender<Work>, notify_receiver: Receiver<Timeout>) {
    let mut list: LinkedList<Timeout> = LinkedList::new();

    loop {
        let mut timeout = match list.pop_front() {
            Some(t) => t,
            None => notify_receiver.recv().expect("Failed to receive timeout"),
        };

        let sleep_time = SystemTime::now();
        match notify_receiver.recv_timeout(timeout.delay) {
            /* We didn't get to wait, let's reduce the time of this work
             * by the amount of time waited so far.
             */
            Ok(new_timeout) => {
                let slept = sleep_time.elapsed().unwrap();
                timeout.delay -= slept;
                list.push_front(timeout);
                timeouts_add_timeout(&mut list, new_timeout);
            }

            /* Timed out, let's process the work and continue */
            Err(_) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("Time went backwards")
                    .as_millis();

                work_sender
                    .send(timeout.work)
                    .expect("Failed to send delayed work");

                println!(
                    "Expected expiration {}",
                    timeout.dbg_expected_trigger.as_millis()
                );
                println!("Actual expiration {}", now);
                if now < timeout.dbg_expected_trigger.as_millis() {
                    println!("TOO EARLY");
                } else {
                    println!(
                        "Target missed by {:?}ms",
                        now - timeout.dbg_expected_trigger.as_millis()
                    );
                }
            }
        }
    }
}

fn worker_thread(work_receiver: Receiver<Work>) {
    loop {
        match work_receiver.recv() {
            Ok(mut work) => work(),
            Err(e) => {
                println!("{:?}", e);
                break;
            }
        }
    }
}

fn main() {
    let (work_sender, work_receiver) = channel();
    let (timeout_work_sender, timeout_work_receiver) = channel();

    /* Startup work processor */
    let worker_thread = thread::spawn(|| worker_thread(work_receiver));

    /* Startup timekeeper for delayed work */
    {
        let work_sender = work_sender.clone();
        thread::spawn(|| timekeeper_thread(work_sender, timeout_work_receiver));
    }

    thread::sleep(Duration::from_millis(100));

    work_sender.send(Box::new(|| work_a(64))).unwrap();

    println!(
        "Start millis: {}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis()
    );

    work_sender
        .send(Box::new(|| work_b("From main".to_string())))
        .unwrap();

    timeout_work_sender
        .send(Timeout::new(
            Box::new(|| {
                work_b("Hello, 200ms later!".to_string());
            }),
            Duration::from_millis(200),
        ))
        .unwrap();
    timeout_work_sender
        .send(Timeout::new(
            Box::new(|| {
                work_b("Hello, 50ms later!".to_string());
            }),
            Duration::from_millis(50),
        ))
        .unwrap();
    timeout_work_sender
        .send(Timeout::new(
            Box::new(|| {
                work_b("Hello, 100ms later!".to_string());
            }),
            Duration::from_millis(100),
        ))
        .unwrap();

    thread::sleep(Duration::from_millis(10));

    timeout_work_sender
        .send(Timeout::new(
            Box::new(|| {
                work_b("Hello, 20ms later!".to_string());
            }),
            Duration::from_millis(20),
        ))
        .unwrap();

    worker_thread.join().unwrap()
}

/* Some work handlers to be executed */

fn work_a(i: u64) {
    let start = SystemTime::now();
    let current_millis = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis();

    println!("a {}: {}", i, current_millis);
}

fn work_b(msg: String) {
    println!("{}", msg);
}
