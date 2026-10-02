//! Throughput measurement. Run with
//! `cargo test --release -p vole-isa-scalar --test throughput -- --ignored --nocapture`.
use vole_core::{Architecture, Machine};
use vole_isa_scalar::{ScalarMachine, assemble};

const STEPS: u64 = 1_000_000;

fn measure(architecture: Architecture, source: &str) {
    let program = assemble(architecture, source).unwrap();
    let mut machine = ScalarMachine::new(architecture);
    machine.load(&program).unwrap();
    let start = std::time::Instant::now();
    for _ in 0..STEPS {
        machine.step().unwrap();
    }
    let seconds = start.elapsed().as_secs_f64();
    println!(
        "{:?}: {STEPS} steps in {seconds:.2}s = {:.0} steps/s",
        architecture,
        STEPS as f64 / seconds
    );
}

#[test]
#[ignore = "timing measurement"]
fn one_million_instruction_loops() {
    measure(
        Architecture::Arm64,
        ".text\n.global _start\n_start:\nmov x0, #0\nloop:\nadd x0, x0, #1\nldr x1, [sp, #-16]\ncmp x0, x2\nb.ne loop\nbrk #0\n",
    );
    measure(
        Architecture::Arm32,
        ".syntax unified\n.text\n.global _start\n_start:\nmov r0, #0\nloop:\nadd r0, r0, #1\nldr r1, [sp, #-16]\ncmp r0, r2\nbne loop\nbkpt #0\n",
    );
    measure(
        Architecture::X86,
        ".intel_syntax noprefix\n.text\n.global _start\n_start:\nmov eax, 0\nloop:\nadd eax, 1\nmov ecx, dword ptr [esp - 16]\ncmp eax, edx\njne loop\nint3\n",
    );
    measure(
        Architecture::X64,
        ".intel_syntax noprefix\n.text\n.global _start\n_start:\nmov eax, 0\nloop:\nadd rax, 1\nmov rcx, qword ptr [rsp - 16]\ncmp rax, rdx\njne loop\nint3\n",
    );
}
