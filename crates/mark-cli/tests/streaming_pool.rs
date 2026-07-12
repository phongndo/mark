use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn non_tty_streaming_path_does_not_start_cpu_pool() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mark"))
        .arg("pager")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mark pager should start");
    let mut stdin = child.stdin.take().expect("stdin should be piped");
    stdin
        .write_all(&vec![b'x'; 256 * 1024])
        .expect("streaming input should write");
    drop(stdin);

    let output = child.wait_with_output().expect("mark pager should finish");
    assert!(
        output.status.success(),
        "streaming path failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
