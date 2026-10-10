//! Frame-time measurement. On Windows, an ETW real-time session listens to the DXGI and
//! Direct3D 9 `Present_Start` events (the same events PresentMon uses) and keeps statistics
//! for one process. Nothing is injected into the game.
//!
//!
//! Games that present another way (Vulkan, OpenGL, older DirectX) are counted from the
//! Microsoft-Windows-DxgKrnl events every present passes through in the kernel, as PresentMon
//! does for them.

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

/// Presents from one event source, timestamps in QPC ticks.
#[derive(Debug, Default)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Stream {
    last: Option<i64>,
    stats: FrameStats,
    /// Timestamps of the last ~2 s of presents, for the live FPS.
    recent: std::collections::VecDeque<i64>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Stream {
    fn add(&mut self, ts: i64, freq: f64) {
        if let Some(prev) = self.last {
            let ms = (ts - prev) as f64 * 1000.0 / freq;
            // Ignore gaps from alt-tab or loading screens.
            if ms < 1000.0 {
                self.stats.add(ms);
            }
        }
        self.last = Some(ts);
        self.recent.push_back(ts);
        let window = (freq * 2.0) as i64;
        while self.recent.front().is_some_and(|t| ts - *t > window) {
            self.recent.pop_front();
        }
    }

    fn live_fps(&self, freq: f64) -> f64 {
        match (self.recent.front(), self.recent.back()) {
            (Some(a), Some(b)) if b > a && self.recent.len() > 1 => (self.recent.len() - 1) as f64 * freq / (*b - *a) as f64,
            _ => 0.0,
        }
    }
}

/// DXGI/D3D9 events are exact for DirectX games; kernel events cover Vulkan and OpenGL. Use the
/// runtime count unless the kernel saw clearly more frames (the game presents another way).
#[cfg_attr(not(windows), allow(dead_code))]
fn pick<'a>(runtime: &'a Stream, kernel: &'a Stream) -> &'a Stream {
    if runtime.stats.frames * 2 >= kernel.stats.frames && runtime.stats.frames > 0 {
        runtime
    } else {
        kernel
    }
}

#[cfg(windows)]
pub use capture::{stop_leftover, PresentCapture, Sources};

#[cfg(windows)]
mod capture {
    use super::{pick, FrameStats, Stream};
    use ::windows::core::{GUID, HSTRING, PWSTR};
    use ::windows::Win32::System::Diagnostics::Etw::*;
    use ::windows::Win32::System::Performance::QueryPerformanceFrequency;
    use std::sync::Mutex;

    const SESSION: &str = "LeanFPS Present";
    // Provider GUIDs and event ids as used by PresentMon (MIT), PresentData/ETW/*.h.
    const DXGI: GUID = GUID::from_u128(0xca11c036_0102_4a2d_a6ad_f03cfed5d3c9);
    const D3D9: GUID = GUID::from_u128(0x783aca0a_790e_4d7f_8451_aa850511c6b9);
    const DXGKRNL: GUID = GUID::from_u128(0x802ec45a_1e99_4b83_9920_87c98277ba9d);
    const DXGI_PRESENT_START: u16 = 42;
    const D3D9_PRESENT_START: u16 = 1;
    // Kernel side of every present, whatever API made it (Vulkan, OpenGL, DirectX):
    // a copy to the screen (blt), a fullscreen flip, or a windowed present handed to DWM.
    const DXGK_BLIT: u16 = 166;
    const DXGK_FLIP: u16 = 168;
    const DXGK_PRESENT_HISTORY_START: u16 = 171;
    const DXGK_KEYWORDS: u64 = 0x1 | 0x800_0000; // Base | Present (Windows 11 adds Present)
    /// PresentHistory model for plain GDI window updates: not a frame.
    const PM_REDIRECTED_GDI: u32 = 1;

    /// Which event sources to count. Both by default; the selftest checks each on its own.
    #[derive(Debug, Clone, Copy)]
    pub struct Sources {
        pub runtime: bool,
        pub kernel: bool,
    }

    impl Default for Sources {
        fn default() -> Self {
            Sources { runtime: true, kernel: true }
        }
    }

    struct State {
        pid: u32,
        freq: f64,
        runtime: Stream,
        kernel: Stream,
        /// Last kernel present (thread, timestamp): one present can show up as several
        /// kernel events on the same thread within microseconds.
        kernel_last: Option<(u32, i64)>,
    }

    static STATE: Mutex<Option<State>> = Mutex::new(None);

    /// The `Model` field of a PresentHistory event, read through the event's own schema.
    unsafe fn present_model(rec: *mut EVENT_RECORD) -> Option<u32> {
        let name: Vec<u16> = "Model".encode_utf16().chain([0]).collect();
        let desc = [PROPERTY_DATA_DESCRIPTOR { PropertyName: name.as_ptr() as u64, ArrayIndex: u32::MAX, Reserved: 0 }];
        let mut buf = [0u8; 4];
        (TdhGetProperty(rec, None, &desc, &mut buf) == 0).then(|| u32::from_le_bytes(buf))
    }

    unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
        let h = &(*rec).EventHeader;
        let id = h.EventDescriptor.Id;
        let runtime = (h.ProviderId == DXGI && id == DXGI_PRESENT_START) || (h.ProviderId == D3D9 && id == D3D9_PRESENT_START);
        let kernel = h.ProviderId == DXGKRNL && matches!(id, DXGK_BLIT | DXGK_FLIP | DXGK_PRESENT_HISTORY_START);
        if !runtime && !kernel {
            return;
        }
        let Ok(mut g) = STATE.lock() else { return };
        let Some(s) = g.as_mut() else { return };
        if h.ProcessId != s.pid {
            return;
        }
        let ts = h.TimeStamp;
        if runtime {
            s.runtime.add(ts, s.freq);
            return;
        }
        if id == DXGK_PRESENT_HISTORY_START && present_model(rec) == Some(PM_REDIRECTED_GDI) {
            return;
        }
        let same_present = s.kernel_last.is_some_and(|(tid, t)| tid == h.ThreadId && (ts - t) as f64 * 1000.0 / s.freq < 0.5);
        s.kernel_last = Some((h.ThreadId, ts));
        if !same_present {
            s.kernel.add(ts, s.freq);
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

    /// Enable a provider with filters so Windows drops unwanted events before they reach us
    /// (the callback filters again, so a Windows version without filter support still works).
    fn enable(handle: CONTROLTRACE_HANDLE, provider: &GUID, level: u8, keywords: u64, filters: &mut [EVENT_FILTER_DESCRIPTOR]) {
        unsafe {
            let params = ENABLE_TRACE_PARAMETERS {
                Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
                EnableFilterDesc: filters.as_mut_ptr(),
                FilterDescCount: filters.len() as u32,
                ..Default::default()
            };
            let r = EnableTraceEx2(handle, provider, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, level, keywords, 0, 0, Some(&params));
            if r.is_err() {
                let _ = EnableTraceEx2(handle, provider, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, level, keywords, 0, 0, None);
            }
        }
    }

    pub struct PresentCapture {
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl PresentCapture {
        /// Start counting presents of `pid`. Needs administrator rights.
        pub fn start(pid: u32) -> Result<PresentCapture, String> {
            PresentCapture::start_with(pid, Sources::default())
        }

        pub fn start_with(pid: u32, sources: Sources) -> Result<PresentCapture, String> {
            stop_session(); // a session left over from a crash
            let mut freq = 0i64;
            unsafe { QueryPerformanceFrequency(&mut freq) }.map_err(|e| e.message().to_string())?;
            *STATE.lock().unwrap() =
                Some(State { pid, freq: freq as f64, runtime: Stream::default(), kernel: Stream::default(), kernel_last: None });

            let mut buf = props_buf();
            let mut handle = CONTROLTRACE_HANDLE::default();
            let r = unsafe { StartTraceW(&mut handle, &HSTRING::from(SESSION), buf.as_mut_ptr() as *mut _) };
            if r.is_err() {
                return Err(format!("frame counter unavailable ({})", r.0));
            }
            if sources.runtime {
                // DXGI and D3D9 are user-mode providers: Windows can filter them to the game's PID.
                let pids = [pid];
                let mut f = [EVENT_FILTER_DESCRIPTOR { Ptr: pids.as_ptr() as u64, Size: 4, Type: EVENT_FILTER_TYPE_PID }];
                for provider in [DXGI, D3D9] {
                    enable(handle, &provider, TRACE_LEVEL_VERBOSE as u8, 0, &mut f);
                }
            }
            if sources.kernel {
                // EVENT_FILTER_EVENT_ID: FilterIn, Reserved, Count, then the ids.
                let ids = [DXGK_BLIT, DXGK_FLIP, DXGK_PRESENT_HISTORY_START];
                let mut data = vec![1u8, 0];
                data.extend_from_slice(&(ids.len() as u16).to_le_bytes());
                for i in ids {
                    data.extend_from_slice(&i.to_le_bytes());
                }
                let mut f = [EVENT_FILTER_DESCRIPTOR { Ptr: data.as_ptr() as u64, Size: data.len() as u32, Type: EVENT_FILTER_TYPE_EVENT_ID }];
                enable(handle, &DXGKRNL, TRACE_LEVEL_INFORMATION as u8, DXGK_KEYWORDS, &mut f);
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
            STATE.lock().unwrap().as_ref().map(|s| pick(&s.runtime, &s.kernel).stats.clone()).unwrap_or_default()
        }

        /// Frames per second over the last ~2 seconds.
        pub fn live_fps(&self) -> f64 {
            STATE.lock().unwrap().as_ref().map(|s| pick(&s.runtime, &s.kernel).live_fps(s.freq)).unwrap_or(0.0)
        }

        /// Stop the session and return the final statistics.
        pub fn finish(mut self) -> FrameStats {
            stop_session();
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            STATE.lock().unwrap().take().map(|s| pick(&s.runtime, &s.kernel).stats.clone()).unwrap_or_default()
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
    fn kernel_count_is_used_only_when_the_runtime_misses_frames() {
        let freq = 1000.0; // 1 tick = 1 ms
        let (mut rt, mut k) = (Stream::default(), Stream::default());
        for i in 0..100 {
            k.add(i * 10, freq);
        }
        assert!(std::ptr::eq(pick(&rt, &k), &k), "Vulkan/OpenGL game: only kernel events");
        for i in 0..90 {
            rt.add(i * 10, freq);
        }
        assert!(std::ptr::eq(pick(&rt, &k), &rt), "DirectX game: runtime events are exact");
        assert!((k.live_fps(freq) - 100.0).abs() < 1.0);
        assert!((k.stats.avg_fps() - 100.0).abs() < 0.01);
        let mut gap = Stream::default();
        gap.add(0, freq);
        gap.add(5000, freq);
        assert_eq!(gap.stats.frames, 0, "alt-tab pause is not a frame");
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
