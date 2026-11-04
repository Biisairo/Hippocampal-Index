//! 처리할 작업이 있는 동안 시스템 유휴 잠자기를 막는다. (FR-DMN-13)

use std::process::{Child, Command, Stdio};

const CAFFEINATE: &str = "caffeinate";

/// 잠자기 방지 자식 프로세스를 잡고 놓는다. 버려지면 놓는다.
pub struct SleepGuard {
    program: String,
    args: Vec<String>,
    child: Option<Child>,
    spawn_failed: bool,
}

impl SleepGuard {
    /// `caffeinate -i -w <pid>`. `pid` 프로세스가 끝나면 caffeinate도 끝난다.
    pub fn caffeinate(pid: u32) -> Self {
        Self::new(CAFFEINATE, &["-i", "-w", &pid.to_string()])
    }

    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            child: None,
            spawn_failed: false,
        }
    }

    /// `busy`면 잡고(끝난 자식은 다시 띄운다), 아니면 놓는다.
    /// 띄우지 못한 첫 번째 실패만 오류 문구로 돌려준다.
    pub fn update(&mut self, busy: bool) -> Option<String> {
        if !busy {
            self.release();
            return None;
        }
        if let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(None))
        {
            return None;
        }
        self.child = None;
        match Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.child = Some(child);
                self.spawn_failed = false;
                None
            }
            Err(e) if !self.spawn_failed => {
                self.spawn_failed = true;
                Some(format!("prevent sleep: cannot start {}: {e}", self.program))
            }
            Err(_) => None,
        }
    }

    /// 잡고 있는 자식 프로세스의 pid.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        self.release();
    }
}
