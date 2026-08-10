use std::cell::RefCell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttemptEvent {
    StaticSoftLimit,
    SlotAcquired,
    HostMemoryAfterSlot,
}

std::thread_local! {
    static EVENTS: RefCell<Vec<AttemptEvent>> = const { RefCell::new(Vec::new()) };
}

pub(super) struct EventRecorder;

impl EventRecorder {
    pub(super) fn start() -> Self {
        EVENTS.with(|events| events.borrow_mut().clear());
        Self
    }

    pub(super) fn events(&self) -> Vec<AttemptEvent> {
        EVENTS.with(|events| events.borrow().clone())
    }
}

pub(super) fn record(event: AttemptEvent) {
    EVENTS.with(|events| events.borrow_mut().push(event));
}
