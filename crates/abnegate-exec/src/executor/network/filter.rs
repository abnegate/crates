use std::io;

use nix::libc;
use nix::libc::sock_filter;
use tokio::process::Command;

/// `AUDIT_ARCH_X86_64`: `EM_X86_64` marked 64-bit and little-endian.
#[cfg(target_arch = "x86_64")]
const NATIVE_ARCHITECTURE: u32 = 0xC000_003E;
/// `AUDIT_ARCH_AARCH64`: `EM_AARCH64` marked 64-bit and little-endian.
#[cfg(target_arch = "aarch64")]
const NATIVE_ARCHITECTURE: u32 = 0xC000_00B7;
/// `AUDIT_ARCH_RISCV64`: `EM_RISCV` marked 64-bit and little-endian.
#[cfg(target_arch = "riscv64")]
const NATIVE_ARCHITECTURE: u32 = 0xC000_00F3;

/// `__X32_SYSCALL_BIT`: set in the number of every syscall made through the
/// x32 ABI, which shares the x86-64 architecture value.
#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// Where `struct seccomp_data` keeps the syscall number.
const NUMBER_OFFSET: u32 = 0;
/// Where `struct seccomp_data` keeps the architecture.
const ARCHITECTURE_OFFSET: u32 = 4;
/// Where `struct seccomp_data` keeps the low word of the first argument, on
/// a little-endian host.
const FIRST_ARGUMENT_OFFSET: u32 = 16;

/// The socket families a denied command may still open.
const ALLOWED_FAMILIES: &[libc::c_int] = &[libc::AF_UNIX, libc::AF_NETLINK];

/// Syscalls that could make or borrow a network socket without `socket`.
const FORBIDDEN_SYSCALLS: &[libc::c_long] = &[
    libc::SYS_io_uring_setup,
    libc::SYS_ptrace,
    libc::SYS_process_vm_writev,
    libc::SYS_pidfd_getfd,
];

/// Install a seccomp filter in `command`'s process just before it executes,
/// which every process it starts inherits and none can lift: a socket
/// outside [`ALLOWED_FAMILIES`] is refused with `EACCES`, and each of
/// [`FORBIDDEN_SYSCALLS`], each a way to make or borrow a socket around that
/// check, with `EPERM`. A filter that cannot be installed fails the spawn.
pub(super) fn deny(command: &mut Command) {
    let instructions = program();
    let length = u16::try_from(instructions.len()).expect("the filter fits a BPF program");
    // Runs between fork and exec, so it allocates nothing: the program was
    // built before the fork, and `prctl` and `syscall` are bare syscall
    // wrappers.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || {
            let program = libc::sock_fprog {
                len: length,
                filter: instructions.as_ptr().cast_mut(),
            };
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            let installed = libc::syscall(
                libc::SYS_seccomp,
                libc::SECCOMP_SET_MODE_FILTER,
                0,
                &raw const program,
            );
            if installed == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// The filter [`deny`] installs.
fn program() -> Vec<sock_filter> {
    let mut program = vec![
        load(ARCHITECTURE_OFFSET),
        equals(NATIVE_ARCHITECTURE, 1, 0),
        refuse(libc::EACCES),
        load(NUMBER_OFFSET),
    ];
    program.extend(foreign_abi());
    program.extend(socket_families());
    program.extend(forbidden_syscalls());
    program
}

#[cfg(target_arch = "x86_64")]
fn foreign_abi() -> Vec<sock_filter> {
    vec![at_least(X32_SYSCALL_BIT, 0, 1), refuse(libc::EACCES)]
}

#[cfg(not(target_arch = "x86_64"))]
fn foreign_abi() -> Vec<sock_filter> {
    Vec::new()
}

fn socket_families() -> Vec<sock_filter> {
    let families = ALLOWED_FAMILIES.len();
    let mut instructions = vec![
        equals(number(libc::SYS_socket), 0, offset(families + 3)),
        load(FIRST_ARGUMENT_OFFSET),
    ];
    for (index, &family) in ALLOWED_FAMILIES.iter().enumerate() {
        instructions.push(equals(family.unsigned_abs(), offset(families - index), 0));
    }
    instructions.push(refuse(libc::EACCES));
    instructions.push(allow());
    instructions
}

fn forbidden_syscalls() -> Vec<sock_filter> {
    let syscalls = FORBIDDEN_SYSCALLS.len();
    let mut instructions: Vec<sock_filter> = FORBIDDEN_SYSCALLS
        .iter()
        .enumerate()
        .map(|(index, &syscall)| equals(number(syscall), offset(syscalls - index), 0))
        .collect();
    instructions.push(allow());
    instructions.push(refuse(libc::EPERM));
    instructions
}

fn number(syscall: libc::c_long) -> u32 {
    u32::try_from(syscall).expect("a syscall number fits a word")
}

fn offset(instructions: usize) -> u8 {
    u8::try_from(instructions).expect("a jump fits a byte")
}

fn code(parts: u32) -> u16 {
    u16::try_from(parts).expect("a BPF opcode fits a half word")
}

fn load(offset: u32) -> sock_filter {
    statement(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, offset)
}

fn equals(value: u32, matched: u8, unmatched: u8) -> sock_filter {
    jump(libc::BPF_JEQ, value, matched, unmatched)
}

#[cfg(target_arch = "x86_64")]
fn at_least(value: u32, matched: u8, unmatched: u8) -> sock_filter {
    jump(libc::BPF_JGE, value, matched, unmatched)
}

fn allow() -> sock_filter {
    statement(libc::BPF_RET | libc::BPF_K, libc::SECCOMP_RET_ALLOW)
}

fn refuse(errno: libc::c_int) -> sock_filter {
    statement(
        libc::BPF_RET | libc::BPF_K,
        libc::SECCOMP_RET_ERRNO | (errno.unsigned_abs() & libc::SECCOMP_RET_DATA),
    )
}

fn statement(parts: u32, k: u32) -> sock_filter {
    sock_filter {
        code: code(parts),
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(comparison: u32, value: u32, matched: u8, unmatched: u8) -> sock_filter {
    sock_filter {
        code: code(libc::BPF_JMP | comparison | libc::BPF_K),
        jt: matched,
        jf: unmatched,
        k: value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parts of a syscall the filter reads.
    struct Call {
        architecture: u32,
        number: u32,
        first_argument: u32,
    }

    impl Call {
        fn native(syscall: libc::c_long) -> Self {
            Self {
                architecture: NATIVE_ARCHITECTURE,
                number: number(syscall),
                first_argument: 0,
            }
        }

        fn socket(family: libc::c_int) -> Self {
            Self {
                first_argument: family.unsigned_abs(),
                ..Self::native(libc::SYS_socket)
            }
        }

        fn word(&self, offset: u32) -> u32 {
            match offset {
                NUMBER_OFFSET => self.number,
                ARCHITECTURE_OFFSET => self.architecture,
                FIRST_ARGUMENT_OFFSET => self.first_argument,
                _ => panic!("the filter read an unexpected offset {offset}"),
            }
        }
    }

    /// What the kernel would do with `call`, by running the filter over it
    /// the way the kernel's BPF interpreter does, for the instructions the
    /// filter uses.
    fn verdict(call: &Call) -> u32 {
        let program = program();
        let mut accumulator = 0;
        let mut counter = 0;
        loop {
            let instruction = program
                .get(counter)
                .unwrap_or_else(|| panic!("the filter ran off its end at {counter}"));
            let parts = u32::from(instruction.code);
            counter += 1;
            if parts == libc::BPF_LD | libc::BPF_W | libc::BPF_ABS {
                accumulator = call.word(instruction.k);
            } else if parts == libc::BPF_RET | libc::BPF_K {
                return instruction.k;
            } else {
                let taken = if parts == libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K {
                    accumulator == instruction.k
                } else if parts == libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K {
                    accumulator >= instruction.k
                } else {
                    panic!("the filter used an unexpected opcode {parts:#x}");
                };
                let skip = if taken {
                    instruction.jt
                } else {
                    instruction.jf
                };
                counter += usize::from(skip);
            }
        }
    }

    fn refused(errno: libc::c_int) -> u32 {
        libc::SECCOMP_RET_ERRNO | errno.unsigned_abs()
    }

    #[test]
    fn the_filter_fits_a_bpf_program_and_starts_by_checking_the_architecture() {
        let program = program();

        assert!(program.len() <= usize::try_from(libc::BPF_MAXINSNS).unwrap());
        assert_eq!(program[0].k, ARCHITECTURE_OFFSET);
        assert_eq!(program[1].k, NATIVE_ARCHITECTURE);
    }

    #[test]
    fn a_unix_or_netlink_socket_is_allowed() {
        assert_eq!(
            verdict(&Call::socket(libc::AF_UNIX)),
            libc::SECCOMP_RET_ALLOW
        );
        assert_eq!(
            verdict(&Call::socket(libc::AF_NETLINK)),
            libc::SECCOMP_RET_ALLOW
        );
    }

    #[test]
    fn every_other_socket_is_refused() {
        for family in [libc::AF_INET, libc::AF_INET6, libc::AF_PACKET, 0, 0xFFFF] {
            assert_eq!(
                verdict(&Call::socket(family)),
                refused(libc::EACCES),
                "{family}"
            );
        }
    }

    #[test]
    fn a_syscall_that_could_reach_around_the_socket_check_is_refused() {
        for &syscall in FORBIDDEN_SYSCALLS {
            assert_eq!(
                verdict(&Call::native(syscall)),
                refused(libc::EPERM),
                "{syscall}"
            );
        }
    }

    #[test]
    fn an_ordinary_syscall_is_allowed() {
        for syscall in [
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_execve,
            libc::SYS_socketpair,
            libc::SYS_connect,
        ] {
            assert_eq!(
                verdict(&Call::native(syscall)),
                libc::SECCOMP_RET_ALLOW,
                "{syscall}"
            );
        }
    }

    #[test]
    fn a_syscall_from_a_foreign_architecture_is_refused() {
        let call = Call {
            architecture: 0x4000_0003,
            ..Call::native(libc::SYS_read)
        };

        assert_eq!(verdict(&call), refused(libc::EACCES));
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn an_x32_syscall_is_refused() {
        let call = Call::native(libc::SYS_socket);
        let call = Call {
            number: call.number | X32_SYSCALL_BIT,
            first_argument: libc::AF_UNIX.unsigned_abs(),
            ..call
        };

        assert_eq!(verdict(&call), refused(libc::EACCES));
    }
}
