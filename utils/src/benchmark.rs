use std::collections::VecDeque;

use web_time::{Duration, Instant};

pub struct Benchmark {
    items: std::collections::HashMap<&'static str, VecDeque<Duration>, fxhash::FxBuildHasher>,
    capacity: usize,
}

impl Benchmark {
    pub fn new(capacity: usize) -> Self {
        Self {
            items: Default::default(),
            capacity,
        }
    }

    pub fn clear(&mut self) {
        for buffer in self.items.values_mut() {
            buffer.clear();
        }
    }
}

impl Benchmark {
    #[inline]
    pub fn with<R>(&mut self, item: &'static str, f: impl FnOnce() -> R) -> R {
        let start = Instant::now();
        let r = f();
        let duration = start.elapsed();
        let buffer = &mut self
            .items
            .entry(item)
            .or_insert_with(|| VecDeque::with_capacity(self.capacity));
        if buffer.len() == self.capacity {
            buffer.pop_front();
        }
        buffer.push_back(duration);

        r
    }

    pub async fn with_async<R, F>(&mut self, item: &'static str, f: F) -> R
    where
        F: std::future::Future<Output = R>,
    {
        let start = Instant::now();
        let r = f.await;
        let duration = start.elapsed();
        let buffer = &mut self
            .items
            .entry(item)
            .or_insert_with(|| VecDeque::with_capacity(self.capacity));
        if buffer.len() == self.capacity {
            buffer.pop_front();
        }
        buffer.push_back(duration);

        r
    }
}

impl Benchmark {
    pub fn last_time(&self, item: &'static str) -> Option<Time> {
        self.items[item]
            .back()
            .map(|d| Time::from_micros(d.as_micros()))
    }

    pub fn average_time(&self, item: &'static str) -> Option<Time> {
        let buffer = &self.items[item];
        if buffer.is_empty() {
            return None;
        }
        let total_micros: u128 = buffer.iter().map(|d| d.as_micros()).sum();
        let avg_micros = total_micros / (buffer.len() as u128);
        Some(Time::from_micros(avg_micros))
    }
}

impl Benchmark {
    pub fn print(&self) {
        // print!("Benchmarks: ");
        // for &item in BenchmarkItem::all() {
        //     print!(
        //         "| {}: last {}, avr {} ",
        //         item.print_names(),
        //         self.last_time(item)
        //             .map(|t| t.to_string())
        //             .unwrap_or("-".to_string()),
        //         self.average_time(item)
        //             .map(|t| t.to_string())
        //             .unwrap_or("-".to_string())
        //     );
        // }

        print!("Benchmarks: | ");
        for item in self.items.keys() {
            print!(
                "{}: last {}, avr {} ",
                item,
                self.last_time(item)
                    .map(|t| t.to_string())
                    .unwrap_or("-".to_string()),
                self.average_time(item)
                    .map(|t| t.to_string())
                    .unwrap_or("-".to_string())
            );
        }
    }
}

pub enum Time {
    Second(u32),
    Millisecond(u32),
    Microsecond(u32),
}

impl Time {
    pub fn from_micros(micros: u128) -> Self {
        if micros <= 10_000 {
            Time::Microsecond(micros as u32)
        } else if micros <= 10_000_000 {
            Time::Millisecond((micros / 1_000) as u32)
        } else {
            Time::Second((micros / 1_000_000) as u32)
        }
    }
}

impl std::fmt::Display for Time {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Time::Second(time) => write!(f, "{time:>4}s "),
            Time::Millisecond(time) => write!(f, "{time:>4}ms"),
            Time::Microsecond(time) => write!(f, "{time:>4}µs"),
        }
    }
}
