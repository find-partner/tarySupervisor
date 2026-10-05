//! 日志 hub：行缓冲批量广播（HLD §3.3），喂 LogStore 与 Matcher。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::HubMsg;
use crate::events::{CoreEvent, EventBus, LogLine, Stream};
use crate::log::LogStore;
use crate::matcher::MatcherState;

pub(super) async fn hub_task(
    id: String,
    mut rx: mpsc::UnboundedReceiver<HubMsg>,
    store: Arc<LogStore>,
    matcher: Arc<Mutex<MatcherState>>,
    bus: EventBus,
    got_output: Arc<AtomicBool>,
) {
    const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(80);
    let mut batch: Vec<LogLine> = Vec::new();
    let mut matcher_dirty = false;
    let mut last_flush = std::time::Instant::now();

    loop {
        let flush_due = !batch.is_empty() && last_flush.elapsed() >= FLUSH_INTERVAL;
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    None => break,
                    Some(HubMsg::Line(stream, text)) => {
                        got_output.store(true, Ordering::Relaxed);
                        let line = store.push(stream, &text);
                        matcher_dirty |= matcher.lock().unwrap().feed_line(&text);
                        batch.push(line);
                    }
                    Some(HubMsg::System(text)) => {
                        let line = store.push(Stream::Out, &text);
                        batch.push(line);
                    }
                }
            }
            _ = tokio::time::sleep(FLUSH_INTERVAL), if flush_due => {}
        }
        if !batch.is_empty() && (batch.len() >= 64 || last_flush.elapsed() >= FLUSH_INTERVAL) {
            bus.send(CoreEvent::LogAppended { id: id.clone(), lines: std::mem::take(&mut batch) });
            if matcher_dirty {
                matcher_dirty = false;
                let items = matcher.lock().unwrap().items();
                bus.send(CoreEvent::MatcherHit { svc_id: id.clone(), items });
            }
            last_flush = std::time::Instant::now();
        }
    }
    if !batch.is_empty() {
        bus.send(CoreEvent::LogAppended { id, lines: batch });
    }
}
