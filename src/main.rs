#![feature(linked_list_cursors)]
#![feature(linked_list_remove)]
#![feature(duration_zero)]

use std::collections::LinkedList;
use std::sync::mpsc::channel;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type DelayedWorkResult = Result<(), &'static str>;

type Work = Box<dyn FnMut() + Send + 'static>;

type ProtectedList<T> = Arc<Mutex<LinkedList<T>>>;

struct TimeoutWorker<T> {
    timeouts: ProtectedList<T>,
    notify_sender: Sender<()>,
}

struct Timeout {
    id: usize,
    work: Work,
    delay: Duration,
    dbg_init_ticks: Duration,
    dbg_expected_trigger: Duration,
}

impl PartialEq for Timeout {
    fn eq(&self, rhs: &Self) -> bool {
        return self.id == rhs.id;
    }
}

static mut ID_COUNTER: usize = 0;

impl Timeout {
    fn new(work: Work, delay: Duration) -> Timeout {
        let current_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");

        unsafe { ID_COUNTER += 1 };
        Timeout {
            id: unsafe { ID_COUNTER },
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

fn timeouts_remove(id: usize, timeouts: &mut LinkedList<Timeout>) -> Option<Timeout> {
    let mut idx = -1;
    for (i, t) in timeouts.iter().enumerate() {
        if id == t.id {
            idx = i as isize;
            break;
        }
    }
    if idx >= 0 {
        return Some(timeouts.remove(idx as usize));
    }
    return None;
}

fn timeouts_decr_delay(id: usize, change: Duration, timeouts: &mut LinkedList<Timeout>) {
    for t in timeouts.iter_mut() {
        if t.id == id {
            if change > t.delay {
                t.delay = Duration::ZERO;
            } else {
                t.delay -= change;
            }
        }
    }
}

fn timekeeper_thread(
    work_sender: Sender<Work>,
    notify_receiver: Receiver<()>,
    list_protected: ProtectedList<Timeout>,
) {
    loop {
        let timeout_info = {
            let list = list_protected.lock().expect("Failed to lock timeout queue");
            list.front().map(|t| (t.id, t.delay))
        };

        /* We have no items, wait for a notification */
        if timeout_info.is_none() {
            notify_receiver.recv().expect("Fail'd to recv notification");
            continue;
        }

        let (t_id, t_delay) = timeout_info.unwrap();
        let sleep_time = SystemTime::now();

        match notify_receiver.recv_timeout(t_delay) {
            /* We didn't get to wait, let's reduce the time of this work
             * by the amount of time waited so far.
             */
            Ok(_) => {
                let mut list = list_protected.lock().expect("Failed to lock timeout queue");
                let slept = sleep_time.elapsed().unwrap();
                timeouts_decr_delay(t_id, slept, &mut list);
            }

            /* Timed out, let's process the work and continue */
            Err(_) => {
                let mut list = list_protected.lock().expect("Failed to lock timeout queue");
                if let Some(expired) = timeouts_remove(t_id, &mut list) {
                    work_sender
                        .send(expired.work)
                        .expect("Failed to send delayed work");

                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("Time went backwards")
                        .as_millis();

                    println!(
                        "Expected expiration {}",
                        expired.dbg_expected_trigger.as_millis()
                    );
                    println!("Actual expiration {}", now);
                    println!(
                        "Target missed by {}",
                        now - expired.dbg_expected_trigger.as_millis()
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

fn delayed_work_submit(
    work: Work,
    delay: Duration,
    timeout_worker: &mut TimeoutWorker<Timeout>,
) -> DelayedWorkResult {
    let mut list = timeout_worker
        .timeouts
        .lock()
        .or_else(|_| Err("Lock failed"))?;
    let mut new = Timeout::new(work, delay);
    let mut list_cursor = list.cursor_front_mut();

    while let Some(t) = list_cursor.current() {
        if t.delay > new.delay {
            t.delay -= new.delay;
            list_cursor.insert_before(new);

            /* Addition at start of queue, re-trigger scheduling */
            if let Some(1) = list_cursor.index() {
                println!("Queue item added to front of queue!");
                timeout_worker
                    .notify_sender
                    .send(())
                    .expect("Fail'd to send notification");
            }

            return Ok(());
        }

        new.delay -= t.delay;

        list_cursor.move_next();
    }

    list_cursor.insert_after(new);

    /* Addition at start of queue, re-trigger scheduling */
    if let None = list_cursor.index() {
        println!("First queue item was added!");
        timeout_worker
            .notify_sender
            .send(())
            .expect("Fail'd to send notification");
    }

    Ok(())
}

fn main() {
    let (work_sender, work_receiver) = channel();
    let (notify_sender, notify_receiver) = channel();
    let list_lock: ProtectedList<Timeout> = Arc::new(Mutex::new(LinkedList::new()));

    /* Startup work processor */
    let worker_thread = thread::spawn(|| worker_thread(work_receiver));

    /* Startup timekeeper for delayed work */
    {
        let list_lock = list_lock.clone();
        let work_sender = work_sender.clone();
        thread::spawn(|| timekeeper_thread(work_sender, notify_receiver, list_lock));
    }

    let mut timeout_worker = TimeoutWorker {
        timeouts: list_lock,
        notify_sender,
    };

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

    delayed_work_submit(
        Box::new(|| {
            work_b("Hello, 200ms later!".to_string());
        }),
        Duration::from_millis(200),
        &mut timeout_worker,
    )
    .unwrap();
    delayed_work_submit(
        Box::new(|| {
            work_b("Hello, 50ms later!".to_string());
        }),
        Duration::from_millis(50),
        &mut timeout_worker,
    )
    .unwrap();
    delayed_work_submit(
        Box::new(|| {
            work_b("Hello, 100ms later!".to_string());
        }),
        Duration::from_millis(100),
        &mut timeout_worker,
    )
    .unwrap();

    thread::sleep(Duration::from_millis(10));

    delayed_work_submit(
        Box::new(|| {
            work_b("Hello, 20ms later!".to_string());
        }),
        Duration::from_millis(20),
        &mut timeout_worker,
    )
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
