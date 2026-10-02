use std::collections::BTreeMap;
use vole_core::{Architecture, RegisterValue, Snapshot};

fn snapshot(architecture: Architecture, registers: &[(&str, u64)]) -> Snapshot {
    Snapshot {
        architecture,
        pc: 0x1000,
        registers: registers
            .iter()
            .map(|(name, value)| RegisterValue {
                name: (*name).into(),
                value: *value,
                bits: architecture.bits(),
            })
            .collect(),
        flags: BTreeMap::new(),
        memory: vec![],
        output: vec![],
        halted: false,
        steps: 0,
        trace: vec![],
    }
}

#[test]
fn arm64_word_aliases_read_low_bits_and_zero_registers_are_constant() {
    let state = snapshot(
        Architecture::Arm64,
        &[
            ("x0", 0xaabb_ccdd_0000_0000),
            ("x1", 0xaabb_ccdd_1234_5678),
            ("x30", 0x1004),
            ("x29", 0x1ffe0),
            ("sp", 0x1234_5678_8765_4321),
        ],
    );
    assert_eq!(state.register("w0"), Some(0));
    assert_eq!(state.register(" W1 "), Some(0x1234_5678));
    assert_eq!(state.register("x1"), Some(0xaabb_ccdd_1234_5678));
    assert_eq!(state.register("wsp"), Some(0x8765_4321));
    assert_eq!(state.register("FP"), Some(0x1ffe0));
    assert_eq!(state.register("lr"), Some(0x1004));
    assert_eq!(state.register("xzr"), Some(0));
    assert_eq!(state.register("wzr"), Some(0));
    assert_eq!(state.register("w31"), None);
    assert_eq!(state.register("x31"), None);
    assert_eq!(state.register("w32"), None);
    assert_eq!(state.register("eax"), None);
}

#[test]
fn x64_low_and_high_aliases_preserve_the_original_snapshot() {
    let state = snapshot(
        Architecture::X64,
        &[
            ("rax", 0x1122_3344_5566_7788),
            ("rbx", 0x0123_4567_89ab_cdef),
            ("rsp", 0x1234_5678_9abc_def0),
            ("rsi", 0xfedc_ba98_7654_3210),
            ("r8", 0xaabb_ccdd_eeff_0123),
            ("r15", 0x1111_2222_3333_4444),
        ],
    );
    let original = state.clone();
    for (name, value) in [
        ("EAX", 0x5566_7788),
        ("ax", 0x7788),
        ("al", 0x88),
        ("ah", 0x77),
        ("bx", 0xcdef),
        ("bh", 0xcd),
        ("esp", 0x9abc_def0),
        ("sp", 0xdef0),
        ("spl", 0xf0),
        ("sil", 0x10),
        ("r8d", 0xeeff_0123),
        ("r8w", 0x0123),
        ("r8b", 0x23),
        ("r15d", 0x3333_4444),
        ("r15w", 0x4444),
        ("r15b", 0x44),
    ] {
        assert_eq!(state.register(name), Some(value), "{name}");
    }
    assert_eq!(state, original);
    assert_eq!(state.register("r16d"), None);
    assert_eq!(state.register("w0"), None);
}

#[test]
fn x86_has_legacy_aliases_without_x64_only_byte_or_quad_registers() {
    let state = snapshot(
        Architecture::X86,
        &[
            ("eax", 0x89ab_cdef),
            ("ebx", 0x1234_5678),
            ("ecx", 0),
            ("esp", 0x1fff0),
            ("esi", 0x1122_3344),
        ],
    );
    for (name, value) in [
        ("eax", 0x89ab_cdef),
        ("ax", 0xcdef),
        ("al", 0xef),
        ("ah", 0xcd),
        ("bx", 0x5678),
        ("bl", 0x78),
        ("bh", 0x56),
        ("cl", 0),
        ("sp", 0xfff0),
        ("si", 0x3344),
    ] {
        assert_eq!(state.register(name), Some(value), "{name}");
    }
    for name in ["rax", "r8", "r8d", "sil", "spl", "w0", "lr"] {
        assert_eq!(state.register(name), None, "{name}");
    }
}

#[test]
fn arm32_conventional_names_and_pc_alias_resolve_stored_values() {
    let state = snapshot(
        Architecture::Arm32,
        &[
            ("r9", 9),
            ("r10", 10),
            ("r11", 11),
            ("r12", 12),
            ("r13", 0x20000),
            ("r14", 0x1040),
            ("pc", 0x1000),
        ],
    );
    for (name, value) in [
        ("sb", 9),
        ("sl", 10),
        ("fp", 11),
        ("ip", 12),
        ("sp", 0x20000),
        ("LR", 0x1040),
        ("r15", 0x1000),
        ("pc", 0x1000),
    ] {
        assert_eq!(state.register(name), Some(value), "{name}");
    }
    assert_eq!(state.register("x0"), None);
    assert_eq!(state.register("eax"), None);
}

#[test]
fn vole_register_names_are_case_insensitive_hex_names() {
    let state = snapshot(Architecture::Vole, &[("R0", 0), ("RA", 0xab), ("RF", 0x56)]);
    assert_eq!(state.register("r0"), Some(0));
    assert_eq!(state.register("ra"), Some(0xab));
    assert_eq!(state.register("RF"), Some(0x56));
    assert_eq!(state.register("r10"), None);
    assert_eq!(state.register("sp"), None);
}
