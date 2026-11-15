//! A tiny fixed thread pool for blocking network work.

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

type Job = Box<dyn FnOnce() + Send>;

pub struct Pool(Sender<Job>);

impl Pool {
    pub fn new(name: &str, threads: usize) -> Self {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..threads {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("{name}{i}"))
                .stack_size(1024 * 1024)
                .spawn(move || loop {
                    let job = rx.lock().unwrap().recv();
                    match job {
                        Ok(job) => job(),
                        Err(_) => return,
                    }
                })
                .expect("cannot start a worker thread");
        }
        Pool(tx)
    }

    pub fn run(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.0.send(Box::new(job));
    }
}
