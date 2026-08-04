//! Copper Phase 0 capture spike.
//!
//! Throwaway code whose deliverable is evidence and a verdict, not a reusable
//! abstraction. Phase 4 moves the proven mechanisms into the app behind a
//! narrow interface; nothing here is designed for that reuse.
//!
//! Thread topology, and the split is the whole point — it keeps every slow
//! thing off the hook callback:
//!
//! - **Hook thread** ([`hook`]) — installs `WH_KEYBOARD_LL`, pumps messages,
//!   runs the double-tap state machine. Touches nothing else.
//! - **Worker thread** (the harness, or the Tauri probe) — receives triggers,
//!   runs the cascade, owns the clipboard work and the findings sink. Never
//!   initialises COM. Owns the hidden message-only clipboard owner window.
//! - **UIA thread** ([`uia`]) — the only thread that initialises COM (MTA),
//!   created once and replaced if a call has to be abandoned on timeout.

pub mod capture;
pub mod clipboard;
pub mod findings;
pub mod foreground;
pub mod hook;
pub mod uia;
