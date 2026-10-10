//! Frame-time measurement. On Windows, an ETW real-time session listens to the DXGI and
//! Direct3D 9 `Present_Start` events (the same events PresentMon uses) and keeps statistics
//! for one process. Nothing is injected into the game.
//!
//! Limits: games that present without DXGI or D3D9 (some OpenGL/Vulkan paths) are not counted.

use serde::{Deserialize, Serialize};

/// Frame times bucketed at 0.1 ms up to 250 ms: fixed memory however long the game runs.
const BIN_MS: f64 = 0.1;
const BINS: usize = 2500;

#[derive(Debug, Clone)]
pub struct FrameStats {
    pub frames: u64,
    pub total_ms: f64,
    hist: Vec<u32>,
}

impl Default for FrameStats {
    fn default() -> Self {
        FrameStats { frames: 0, total_ms: 0.0, hist: vec![0; BINS] }
    }
}

impl FrameStats {
    pub fn add(&mut self, frame_ms: f64) {
        if !(frame_ms > 0.0) {
            return;
        }
        self.frames += 1;
        self.total_ms += frame_ms;
        let i = ((frame_ms / BIN_MS) as usize).min(BINS - 1);
        self.hist[i] += 1;
    }

    pub fn avg_fps(&self) -> f64 {
        if self.total_ms <= 0.0 {
            0.0
        } else {
            self.frames as f64 * 1000.0 / self.total_ms
        }
    }

    /// FPS at the slowest `pct` percent of frames ("1% low" = pct 1.0), from the frame time
    /// percentile, as PresentMon-based tools report it.
    pub fn low_fps(&self, pct: f64) -> f64 {
        if self.frames == 0 {
            return 0.0;
        }
        let target = ((self.frames as f64) * pct / 100.0).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for i in (0..BINS).rev() {
            seen += self.hist[i] as u64;
            if seen >= target {
                let ms = (i as f64 + 0.5) * BIN_MS;
                return 1000.0 / ms;
            }
        }
        0.0
    }
}

/// One game session as stored in `history.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Lower-case exe file name.
    pub exe: String,
    pub name: String,
    /// Unix seconds.
    pub started: u64,
    pub minutes: f64,
    pub frames: u64,
    pub avg_fps: f64,
    pub low1_fps: f64,
    pub low01_fps: f64,
    /// Session optimizations that were active.
    pub optimizations: usize,
}

impl Record {
    pub fn from_stats(exe: &str, name: &str, started: u64, s: &FrameStats, optimizations: usize) -> Record {
        Record {
            exe: exe.into(),
            name: name.into(),
            started,
            minutes: s.total_ms / 60_000.0,
            frames: s.frames,
            avg_fps: s.avg_fps(),
            low1_fps: s.low_fps(1.0),
            low01_fps: s.low_fps(0.1),
            optimizations,
        }
    }
}

/// Sessions shorter than this (or with too few frames) are not worth comparing.
pub const MIN_RECORD_MINUTES: f64 = 1.0;

/// Average of earlier sessions of the same game, for "vs your usual" comparisons.
pub fn usual(history: &[Record], exe: &str) -> Option<(f64, f64, usize)> {
    let past: Vec<&Record> = history.iter().filter(|r| r.exe == exe && r.minutes >= MIN_RECORD_MINUTES).collect();
    if past.is_empty() {
        return None;
    }
    let n = past.len() as f64;
    Some((past.iter().map(|r| r.avg_fps).sum::<f64>() / n, past.iter().map(|r| r.low1_fps).sum::<f64>() / n, past.len()))
}

#[cfg(windows)]
pub use capture::{stop_leftover, PresentCapture};

#[cfg(windows)]
mod capture {
    use super::FrameStats;
    use ::windows::core::{GUID, HSTRING, PWSTR};
    use ::windows::Win32::System::Diagnostics::Etw::*;
    use ::windows::Win32::System::Performance::QueryPerformanceFrequency;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const SESSION: &str = "LeanFPS Present";
    // Provider GUIDs and event ids as used by PresentMon (MIT), PresentData/ETW/*.h.
    const DXGI: GUID = GUID::from_u128(0xca11c036_0102_4a2d_a6ad_f03cfed5d3c9);
    const D3D9: GUID = GUID::from_u128(0x783aca0a_790e_4d7f_8451_aa850511c6b9);
    const DXGI_PRESENT_START: u16 = 42;
    const D3D9_PRESENT_START: u16 = 1;

    struct State {
        pid: u32,
        freq: f64,
        last: Option<i64>,
        stats: FrameStats,
        /// Timestamps of the last ~2 s of presents, for the live FPS.
        recent: VecDeque<i64>,
    }

    static STATE: Mutex<Option<State>> = Mutex::new(None);

    unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
        let h = &(*rec).EventHeader;
        let id = h.EventDescriptor.Id;
        let present = (h.ProviderId == DXGI && id == DXGI_PRESENT_START) || (h.ProviderId == D3D9 && id == D3D9_PRESENT_START);
        if !present {
            return;
        }
        let Ok(mut g) = STATE.lock() else { return };
        let Some(s) = g.as_mut() else { return };
        if h.ProcessId != s.pid {
            return;
        }
        let ts = h.TimeStamp;
        if let Some(prev) = s.last {
            let ms = (ts - prev) as f64 * 1000.0 / s.freq;
            // Ignore gaps from alt-tab or loading screens.
            if ms < 1000.0 {
                s.stats.add(ms);
            }
        }
        s.last = Some(ts);
        s.recent.push_back(ts);
        let window = (s.freq * 2.0) as i64;
        while s.recent.front().is_some_and(|t| ts - *t > window) {
            s.recent.pop_front();
        }
    }

    fn props_buf() -> Vec<u8> {
        let size = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() + 1024;
        let mut buf = vec![0u8; size];
        let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
        unsafe {
            (*p).Wnode.BufferSize = size as u32;
            (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
            (*p).Wnode.ClientContext = 1; // QueryPerformanceCounter timestamps
            (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
            (*p).LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
        }
        buf
    }

    fn stop_session() {
        let mut buf = props_buf();
        unsafe {
            let _ = ControlTraceW(CONTROLTRACE_HANDLE::default(), &HSTRING::from(SESSION), buf.as_mut_ptr() as *mut _, EVENT_TRACE_CONTROL_STOP);
        }
    }

    /// Stop a frame-counter trace left running by a crash (used by Restore all).
    pub fn stop_leftover() {
        stop_session();
    }

    pub struct PresentCapture {
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl PresentCapture {
        /// Start counting presents of `pid`. Needs administrator rights.
        pub fn start(pid: u32) -> Result<PresentCapture, String> {
            stop_session(); // a session left over from a crash
            let mut freq = 0i64;
            unsafe { QueryPerformanceFrequency(&mut freq) }.map_err(|e| e.message().to_string())?;
            *STATE.lock().unwrap() = Some(State { pid, freq: freq as f64, last: None, stats: FrameStats::default(), recent: VecDeque::new() });

            let mut buf = props_buf();
            let mut handle = CONTROLTRACE_HANDLE::default();
            let r = unsafe { StartTraceW(&mut handle, &HSTRING::from(SESSION), buf.as_mut_ptr() as *mut _) };
            if r.is_err() {
                return Err(format!("frame counter unavailable ({})", r.0));
            }
            for provider in [DXGI, D3D9] {
                unsafe {
                    let _ = EnableTraceEx2(handle, &provider, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, TRACE_LEVEL_VERBOSE as u8, 0, 0, 0, None);
                }
            }
            let thread = std::thread::Builder::new()
                .name("frames".into())
                .spawn(|| unsafe {
                    let mut name: Vec<u16> = SESSION.encode_utf16().chain([0]).collect();
                    let mut log = EVENT_TRACE_LOGFILEW { LoggerName: PWSTR(name.as_mut_ptr()), ..Default::default() };
                    log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
                    log.Anonymous2.EventRecordCallback = Some(on_event);
                    let h = OpenTraceW(&mut log);
                    if h.Value == u64::MAX {
                        return;
                    }
                    // Blocks until the session is stopped.
                    let _ = ProcessTrace(&[h], None, None);
                    let _ = CloseTrace(h);
                })
                .map_err(|e| e.to_string())?;
            Ok(PresentCapture { thread: Some(thread) })
        }

        pub fn stats(&self) -> FrameStats {
            STATE.lock().unwrap().as_ref().map(|s| s.stats.clone()).unwrap_or_default()
        }

        /// Frames per second over the last ~2 seconds.
        pub fn live_fps(&self) -> f64 {
            let g = STATE.lock().unwrap();
            let Some(s) = g.as_ref() else { return 0.0 };
            match (s.recent.front(), s.recent.back()) {
                (Some(a), Some(b)) if b > a && s.recent.len() > 1 => (s.recent.len() - 1) as f64 * s.freq / (*b - *a) as f64,
                _ => 0.0,
            }
        }

        /// Stop the session and return the final statistics.
        pub fn finish(mut self) -> FrameStats {
            stop_session();
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            STATE.lock().unwrap().take().map(|s| s.stats).unwrap_or_default()
        }
    }

    impl Drop for PresentCapture {
        fn drop(&mut self) {
            if self.thread.is_some() {
                stop_session();
                if let Some(t) = self.thread.take() {
                    let _ = t.join();
                }
                STATE.lock().unwrap().take();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_and_stuttery_frames() {
        let mut s = FrameStats::default();
        for _ in 0..990 {
            s.add(1000.0 / 144.0);
        }
        for _ in 0..10 {
            s.add(1000.0 / 30.0);
        }
        let avg = s.avg_fps();
        assert!((avg - 1000.0 / ((990.0 * 6.944 + 10.0 * 33.33) / 1000.0)).abs() < 1.0, "{avg}");
        let low1 = s.low_fps(1.0);
        assert!((low1 - 30.0).abs() < 1.0, "1% low is the 30 FPS stutter: {low1}");
        assert!(s.low_fps(10.0) > 140.0, "10% low is still steady");
        assert_eq!(FrameStats::default().low_fps(1.0), 0.0);
    }

    #[test]
    fn usual_averages_earlier_long_sessions() {
        let mut st = FrameStats::default();
        for _ in 0..6000 {
            st.add(10.0);
        }
        let a = Record::from_stats("cs2.exe", "CS2", 1, &st, 5);
        assert!((a.avg_fps - 100.0).abs() < 0.01 && a.minutes == 1.0);
        let mut short = a.clone();
        short.minutes = 0.2;
        short.avg_fps = 10.0;
        let (avg, _, n) = usual(&[a.clone(), short], "cs2.exe").unwrap();
        assert_eq!((avg.round(), n), (100.0, 1));
        assert!(usual(&[a], "other.exe").is_none());
    }
}
