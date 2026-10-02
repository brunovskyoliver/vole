//! Line-table queries shared by breakpoints and source stepping.
use vole_core::{DebugInfo, debug::LineRow};

/// Furthest a breakpoint on a line without code snaps forward.
const SNAP_LINES: usize = 64;

/// The row whose range contains `address` (not necessarily its start).
pub fn line_at(debug: &DebugInfo, address: u64) -> Option<&LineRow> {
    debug.row_for_address(address)
}

/// The user-file `is_stmt` row that starts exactly at `address`, if any.
/// Source steps stop only at such addresses.
pub fn statement_start(debug: &DebugInfo, address: u64) -> Option<&LineRow> {
    let end = debug.lines.partition_point(|row| row.address <= address);
    let start = debug.lines[..end].partition_point(|row| row.address < address);
    debug.lines[start..end].iter().rev().find(|row| {
        row.is_stmt && !row.end_sequence && row.line != 0 && debug.is_user_file(row.file)
    })
}

/// Resolve a 1-based user source line to a breakpoint address and the line actually used.
///
/// Uses the lowest-address `is_stmt` user row for the line. A function's
/// `low_pc` moves to its `prologue_end` so locals are readable when the
/// breakpoint hits. Lines without code snap forward (at most 64 lines). The
/// returned line is the line of the row at the final address.
pub fn breakpoint_address(debug: &DebugInfo, line: usize) -> Option<(u64, usize)> {
    if line == 0 {
        return None;
    }
    for candidate in line..=line.saturating_add(SNAP_LINES) {
        let lowest = debug
            .lines
            .iter()
            .filter(|row| {
                row.line as usize == candidate
                    && row.is_stmt
                    && !row.end_sequence
                    && debug.is_user_file(row.file)
            })
            .map(|row| row.address)
            .min();
        let Some(mut address) = lowest else {
            continue;
        };
        if let Some(end) = debug
            .functions
            .iter()
            .find(|function| function.low_pc == address)
            .and_then(|function| function.prologue_end)
        {
            address = end;
        }
        let resolved = statement_start(debug, address)
            .or_else(|| line_at(debug, address).filter(|row| debug.is_user_file(row.file)))
            .map(|row| row.line as usize)
            .filter(|resolved| *resolved != 0)
            .unwrap_or(candidate);
        return Some((address, resolved));
    }
    None
}
