//! Structured logging for analysis and human monitoring.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::events::Side;

/// CSV header for log file
const CSV_HEADER: &str = "type,timestamp,time_left,market,mid,var,k,inventory,yes_target,no_target,yes_resting,no_resting,pair_cost,spread,side,price,size,order_id,is_maker,reason,error";

/// Unmatched fill for FIFO pairing
#[allow(dead_code)]
struct UnmatchedFill {
    price: u16,
    size: f64, // Reserved for future partial fill support
}

/// Per-window statistics
#[derive(Default)]
pub struct WindowStats {
    pub yes_fills: u32,
    pub no_fills: u32,
    pub matched_pairs: u32,
    pub pair_costs: Vec<f64>,
    pub unmatched_yes: u32,
    pub unmatched_no: u32,
    pub gross_pnl: f64,
    pub ticks_quoted: u32,
    pub ticks_halted: u32,
    pub toxic_cancels: u32,
    pub stale_halts: u32,
    // For FIFO matching
    yes_queue: VecDeque<UnmatchedFill>,
    no_queue: VecDeque<UnmatchedFill>,
}

impl WindowStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn avg_pair_cost(&self) -> f64 {
        if self.pair_costs.is_empty() {
            0.0
        } else {
            self.pair_costs.iter().sum::<f64>() / self.pair_costs.len() as f64
        }
    }

    /// Record a fill and try to match pairs (FIFO)
    pub fn record_fill(&mut self, side: Side, price: u16) {
        match side {
            Side::Yes => {
                self.yes_fills += 1;
                // Try to match with oldest NO fill
                if let Some(no_fill) = self.no_queue.pop_front() {
                    let pair_cost = (price as f64 + no_fill.price as f64) / 1000.0;
                    self.pair_costs.push(pair_cost);
                    self.matched_pairs += 1;
                    self.gross_pnl += 1.0 - pair_cost;
                } else {
                    // No match, queue this YES fill
                    self.yes_queue.push_back(UnmatchedFill { price, size: 5.0 });
                }
            }
            Side::No => {
                self.no_fills += 1;
                // Try to match with oldest YES fill
                if let Some(yes_fill) = self.yes_queue.pop_front() {
                    let pair_cost = (yes_fill.price as f64 + price as f64) / 1000.0;
                    self.pair_costs.push(pair_cost);
                    self.matched_pairs += 1;
                    self.gross_pnl += 1.0 - pair_cost;
                } else {
                    // No match, queue this NO fill
                    self.no_queue.push_back(UnmatchedFill { price, size: 5.0 });
                }
            }
        }
    }

    /// Finalize window stats (count unmatched)
    pub fn finalize(&mut self) {
        self.unmatched_yes = self.yes_queue.len() as u32;
        self.unmatched_no = self.no_queue.len() as u32;
    }
}

/// Session-wide statistics
#[derive(Default)]
pub struct SessionStats {
    pub start_time: f64,
    pub windows: u32,
    pub yes_fills: u32,
    pub no_fills: u32,
    pub matched_pairs: u32,
    pub pair_costs: Vec<f64>,
    pub unmatched_yes: u32,
    pub unmatched_no: u32,
    pub gross_pnl: f64,
    pub orders_placed: u32,
    pub orders_cancelled: u32,
    pub cancel_fails: u32,
    pub order_fails: u32,
    pub toxic_cancels: u32,
    pub stale_halts: u32,
    pub ticks_total: u32,
    pub ticks_quoted: u32,
}

impl SessionStats {
    pub fn new() -> Self {
        Self {
            start_time: now_secs(),
            ..Default::default()
        }
    }

    pub fn avg_pair_cost(&self) -> f64 {
        if self.pair_costs.is_empty() {
            0.0
        } else {
            self.pair_costs.iter().sum::<f64>() / self.pair_costs.len() as f64
        }
    }

    pub fn merge_window(&mut self, window: &WindowStats) {
        self.windows += 1;
        self.yes_fills += window.yes_fills;
        self.no_fills += window.no_fills;
        self.matched_pairs += window.matched_pairs;
        self.pair_costs.extend(&window.pair_costs);
        self.unmatched_yes += window.unmatched_yes;
        self.unmatched_no += window.unmatched_no;
        self.gross_pnl += window.gross_pnl;
        self.ticks_total += window.ticks_quoted + window.ticks_halted;
        self.ticks_quoted += window.ticks_quoted;
        self.toxic_cancels += window.toxic_cancels;
        self.stale_halts += window.stale_halts;
    }

    pub fn duration_str(&self) -> String {
        let secs = (now_secs() - self.start_time) as u64;
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if hours > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}m", mins)
        }
    }

    pub fn pct_quoted(&self) -> f64 {
        if self.ticks_total == 0 {
            0.0
        } else {
            self.ticks_quoted as f64 / self.ticks_total as f64 * 100.0
        }
    }
}

/// Logger handles both CSV file and stdout
pub struct Logger {
    file: File,
    last_quote_log: f64,  // Throttle stdout to 1/sec
}

impl Logger {
    pub fn new() -> anyhow::Result<Self> {
        fs::create_dir_all("logs")?;
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let path = format!("logs/polybot_{}.csv", ts);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        // Write header
        writeln!(file, "{}", CSV_HEADER)?;
        println!("[LOG] Writing to {}", path);

        Ok(Self {
            file,
            last_quote_log: 0.0,
        })
    }

    fn timestamp_str(&self) -> String {
        let now = chrono::Local::now();
        now.format("%H:%M:%S").to_string()
    }

    /// Log a TICK event (every 50ms while quoting)
    pub fn tick(
        &mut self,
        time_left: f64,
        market: &str,
        mid: f64,
        var: f64,
        k: f64,
        inventory: f64,
        yes_target: u16,
        no_target: u16,
        yes_resting: u16,
        no_resting: u16,
    ) {
        let now = now_secs();
        let pair_cost = (yes_target + no_target) as f64 / 1000.0;
        let spread = 1.0 - pair_cost;

        // CSV
        writeln!(
            self.file,
            "TICK,{:.3},{:.1},{},{:.4},{:.6},{:.4},{:.1},{},{},{},{},{:.4},{:.4},,,,,,",
            now, time_left, market, mid, var, k, inventory,
            yes_target, no_target, yes_resting, no_resting, pair_cost, spread
        ).ok();

        // Stdout (throttled to 1/sec)
        if now - self.last_quote_log >= 1.0 {
            self.last_quote_log = now;
            println!(
                "[{}] QUOTE | T-{:.0} mid={:.2} var={:.4} k={:.1} inv={:.0} | Y={}¢ N={}¢ | pair={:.4} spd={:.2}%",
                self.timestamp_str(),
                time_left,
                mid,
                var,
                k,
                inventory,
                yes_target / 10,
                no_target / 10,
                pair_cost,
                spread * 100.0
            );
        }
    }

    /// Log a HALT event
    pub fn halt(&mut self, time_left: f64, market: &str, mid: f64, var: f64, reason: &str, cancelled_count: usize) {
        let now = now_secs();

        // CSV
        writeln!(
            self.file,
            "HALT,{:.3},{:.1},{},{:.4},{:.6},,,,,,,,,,,,,,{},",
            now, time_left, market, mid, var, reason
        ).ok();

        // Stdout
        if cancelled_count > 0 {
            println!(
                "[{}] HALT {} | cancelled {} orders",
                self.timestamp_str(), reason, cancelled_count
            );
        }
    }

    /// Log ORDER (successful placement)
    pub fn order(&mut self, time_left: f64, market: &str, side: Side, price: u16, size: f64, order_id: &str) {
        let now = now_secs();
        let side_str = if side == Side::Yes { "YES" } else { "NO" };

        // CSV
        writeln!(
            self.file,
            "ORDER,{:.3},{:.1},{},,,,,,,,,,,,{},{},{:.1},{},,,",
            now, time_left, market, side_str, price, size, order_id
        ).ok();

        // Stdout
        println!(
            "[{}] ORDER {} {}@{}¢ → ord:{}",
            self.timestamp_str(), side_str, size as u32, price / 10, &order_id[..order_id.len().min(8)]
        );
    }

    /// Log ORDER_FAIL
    pub fn order_fail(&mut self, time_left: f64, market: &str, side: Side, price: u16, size: f64, error: &str) {
        let now = now_secs();
        let side_str = if side == Side::Yes { "YES" } else { "NO" };

        // CSV
        writeln!(
            self.file,
            "ORDER_FAIL,{:.3},{:.1},{},,,,,,,,,,,,{},{},{:.1},,,,{}",
            now, time_left, market, side_str, price, size, error
        ).ok();

        // Stdout
        println!(
            "[{}] ORDER_FAIL {} {}@{}¢: {}",
            self.timestamp_str(), side_str, size as u32, price / 10, error
        );
    }

    /// Log CANCEL
    pub fn cancel(&mut self, time_left: f64, market: &str, order_id: &str, reason: &str) {
        let now = now_secs();

        // CSV
        writeln!(
            self.file,
            "CANCEL,{:.3},{:.1},{},,,,,,,,,,,,,,,{},{},",
            now, time_left, market, order_id, reason
        ).ok();

        // Stdout (only for interesting reasons)
        if reason != "REPLACE" {
            println!(
                "[{}] CANCEL {} ({})",
                self.timestamp_str(), &order_id[..order_id.len().min(8)], reason
            );
        }
    }

    /// Log CANCEL_FAIL
    pub fn cancel_fail(&mut self, time_left: f64, market: &str, order_id: &str, error: &str) {
        let now = now_secs();

        // CSV
        writeln!(
            self.file,
            "CANCEL_FAIL,{:.3},{:.1},{},,,,,,,,,,,,,,,{},,{}",
            now, time_left, market, order_id, error
        ).ok();

        // Stdout
        println!(
            "[{}] CANCEL_FAIL {}: {}",
            self.timestamp_str(), &order_id[..order_id.len().min(8)], error
        );
    }

    /// Log FILL
    pub fn fill(
        &mut self,
        time_left: f64,
        market: &str,
        side: Side,
        price: u16,
        size: f64,
        order_id: &str,
        is_maker: bool,
        inventory_yes: f64,
        inventory_no: f64,
        pair_cost: Option<f64>,
        pnl: f64,
    ) {
        let now = now_secs();
        let side_str = if side == Side::Yes { "YES" } else { "NO" };
        let inventory = inventory_yes - inventory_no;

        // CSV
        writeln!(
            self.file,
            "FILL,{:.3},{:.1},{},,,,{:.1},,,,,,,{},{},{:.1},{},{},,,",
            now, time_left, market, inventory, side_str, price, size, order_id, is_maker
        ).ok();

        // Stdout
        let maker_str = if is_maker { "MAKER" } else { "TAKER" };
        if let Some(pc) = pair_cost {
            println!(
                "[{}] FILL {} {} {}@{}¢ ({}) | inv: Y={:.0} N={:.0} | pair={:.4} pnl=${:.2}",
                self.timestamp_str(), maker_str, side_str, size as u32, price / 10,
                &order_id[..order_id.len().min(8)], inventory_yes, inventory_no, pc, pnl
            );
        } else {
            println!(
                "[{}] FILL {} {} {}@{}¢ ({}) | inv: Y={:.0} N={:.0} | pnl=${:.2}",
                self.timestamp_str(), maker_str, side_str, size as u32, price / 10,
                &order_id[..order_id.len().min(8)], inventory_yes, inventory_no, pnl
            );
        }
    }

    /// Log TOXIC
    pub fn toxic(&mut self, time_left: f64, market: &str, btc_from: f64, btc_to: f64, duration: f64, cancelled_count: usize) {
        let now = now_secs();
        let reason = format!("${:.0}→${:.0} in {:.1}s", btc_from, btc_to, duration);

        // CSV
        writeln!(
            self.file,
            "TOXIC,{:.3},{:.1},{},,,,,,,,,,,,,,,,,{},",
            now, time_left, market, reason
        ).ok();

        // Stdout
        println!(
            "[{}] BTC CRASH {} | cancelled {} orders",
            self.timestamp_str(), reason, cancelled_count
        );
    }

    /// Log WINDOW_START
    pub fn window_start(&mut self, market: &str) {
        let now = now_secs();

        // CSV
        writeln!(self.file, "WINDOW_START,{:.3},,{},,,,,,,,,,,,,,,,,,", now, market).ok();

        // Stdout
        println!(">>> WINDOW START {}", market);
    }

    /// Log WINDOW_END with stats, position, and parameters
    pub fn window_end(
        &mut self,
        market: &str,
        stats: &WindowStats,
        yes_shares: f64,
        yes_avg_price: f64,
        no_shares: f64,
        no_avg_price: f64,
        min_pnl: f64,
        gamma: f64,
        halt_secs: f64,
    ) {
        let now = now_secs();

        // CSV - include position and params
        writeln!(
            self.file,
            "WINDOW_END,{:.3},,{},Y={:.1}@{:.1}c,N={:.1}@{:.1}c,minpnl=${:.2},gamma={:.4},halt={},,,,,,,,,,,,",
            now, market, yes_shares, yes_avg_price, no_shares, no_avg_price, min_pnl, gamma, halt_secs
        ).ok();

        // Stdout
        println!(">>> WINDOW END {}", market);
        println!(
            "    position: Y={:.0}@{:.1}¢ N={:.0}@{:.1}¢ | min_pnl=${:.2}",
            yes_shares, yes_avg_price, no_shares, no_avg_price, min_pnl
        );
        println!(
            "    fills: Y={} N={} | pairs={} | avg_pair={:.4}",
            stats.yes_fills, stats.no_fills, stats.matched_pairs, stats.avg_pair_cost()
        );
        println!(
            "    unmatched: Y={} N={} | gross=${:.2}",
            stats.unmatched_yes, stats.unmatched_no, stats.gross_pnl
        );
        println!(
            "    quoted={}/{} ticks | toxic={} stale={}",
            stats.ticks_quoted,
            stats.ticks_quoted + stats.ticks_halted,
            stats.toxic_cancels,
            stats.stale_halts
        );
        println!("    params: gamma={:.4} halt_secs={:.0}", gamma, halt_secs);
    }

    /// Log session summary on shutdown
    pub fn session_summary(&mut self, stats: &SessionStats) {
        println!("\n=== SESSION SUMMARY ===");
        println!("Duration: {}", stats.duration_str());
        println!("Windows: {}", stats.windows);
        println!("Total fills: Y={} N={}", stats.yes_fills, stats.no_fills);
        println!("Matched pairs: {}", stats.matched_pairs);
        println!("Avg pair cost: {:.4}", stats.avg_pair_cost());
        println!("Unmatched: Y={} N={}", stats.unmatched_yes, stats.unmatched_no);
        println!("Gross PnL: ${:.2}", stats.gross_pnl);
        println!("Orders placed: {}", stats.orders_placed);
        println!("Orders cancelled: {}", stats.orders_cancelled);
        println!("Cancel fails: {}", stats.cancel_fails);
        println!("Order fails: {}", stats.order_fails);
        println!("Toxic cancels: {}", stats.toxic_cancels);
        println!("Stale halts: {}", stats.stale_halts);
        println!("Avg ticks quoted: {:.0}%", stats.pct_quoted());
    }

    /// Log price replacement (stdout only)
    pub fn replace(&mut self, side: Side, old_price: u16, new_price: u16) {
        let side_str = if side == Side::Yes { "YES" } else { "NO" };
        println!(
            "[{}] REPLACE {} {}¢→{}¢",
            self.timestamp_str(), side_str, old_price / 10, new_price / 10
        );
    }

    /// Flush the file
    pub fn flush(&mut self) {
        self.file.flush().ok();
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}
