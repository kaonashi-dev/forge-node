use super::*;

struct Run {
    _dir: tempfile::TempDir,
    policy: Policy,
}

fn run() -> Run {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical");
    let run_dir = root.join("run");
    let cache_dir = root.join("cache");
    let tmp_dir = root.join("tmp");
    let forge = root.join("forge-home");
    for path in [&run_dir, &cache_dir, &tmp_dir, &forge] {
        std::fs::create_dir_all(path).expect("dir");
    }
    std::fs::write(forge.join("secret"), "token").expect("secret");
    Run {
        _dir: dir,
        policy: Policy {
            run_dir,
            cache_dir,
            tmp_dir,
            denied_reads: vec![forge],
            network: false,
            socket: None,
        },
    }
}

fn target(command: &str) -> Target {
    Target {
        name: "t".into(),
        command: command.into(),
    }
}

fn env() -> Vec<(String, String)> {
    vec![("PATH".into(), "/usr/bin:/bin".into())]
}

fn go(run: &Run, command: &str) -> TargetResult {
    run_target(
        &target(command),
        Duration::from_secs(30),
        &run.policy,
        &env(),
    )
    .expect("ran")
}

#[test]
fn checks_toml_parses_and_bad_files_are_explained() {
    let config = parse(
        "timeout_secs = 9000\n[env]\nRUST_BACKTRACE = \"1\"\n[[targets]]\nname = \"test\"\ncommand = \"cargo test\"\n",
    )
    .expect("valid");
    assert_eq!(config.timeout, Duration::from_secs(3600), "capped");
    assert_eq!(config.env, [("RUST_BACKTRACE".to_owned(), "1".to_owned())]);
    assert_eq!(
        config.targets,
        [Target {
            name: "test".into(),
            command: "cargo test".into()
        }]
    );
    assert_eq!(
        parse("[[targets]]\nname = \"a\"\ncommand = \"true\"\n")
            .expect("ok")
            .timeout,
        Duration::from_secs(600)
    );
    assert!(parse("").unwrap_err().contains("no `[[targets]]`"));
    assert!(parse("[[targets]]\nname = \"a\"\n")
        .unwrap_err()
        .contains("`command`"));
    assert!(parse(
        "[[targets]]\nname = \"a\"\ncommand = \"b\"\n[[targets]]\nname = \"a\"\ncommand = \"c\"\n"
    )
    .unwrap_err()
    .contains("twice"));
}

#[test]
fn the_environment_is_the_whitelist_the_file_and_the_cache_only() {
    let resolved = |name: &str| match name {
        "PATH" => Some("/usr/bin".to_owned()),
        "HOME" => Some("/Users/me".to_owned()),
        "FORGE_SOCKET" => Some("/forge.sock".to_owned()),
        _ => None,
    };
    let env = environment(
        &["PATH".into(), "FORGE_SOCKET".into(), "MISSING".into()],
        &resolved,
        &[("PATH".into(), "/opt/bin".into())],
        Path::new("/c"),
    );
    assert_eq!(
        env,
        [
            ("PATH".to_owned(), "/opt/bin".to_owned()),
            ("FORGE_SANDBOX_CACHE".to_owned(), "/c".to_owned())
        ]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_passing_and_a_failing_target_report_their_exit_and_output() {
    let run = run();
    let passed = go(&run, "echo hello; echo oops >&2");
    assert!(passed.passed());
    assert_eq!(
        (passed.stdout.as_str(), passed.stderr.as_str()),
        ("hello\n", "oops\n")
    );
    let failed = go(&run, "echo broke; exit 3");
    assert_eq!(failed.exit_code, Some(3));
    assert!(!failed.timed_out);
    let long = go(
        &run,
        "i=0; while [ $i -lt 3000 ]; do echo line-$i; i=$((i+1)); done",
    );
    assert!(long.stdout.len() <= TAIL_BYTES);
    assert!(long.stdout.ends_with("line-2999\n"), "the tail is kept");
}

#[cfg(target_os = "macos")]
#[test]
fn a_target_cannot_reach_the_network() {
    let run = run();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = listener.local_addr().expect("addr").port();
    let probe = format!("/usr/bin/nc -z -w 2 127.0.0.1 {port}");
    let outside = Command::new("/bin/sh")
        .arg("-c")
        .arg(&probe)
        .status()
        .expect("nc");
    assert!(outside.success(), "the probe connects without the sandbox");
    assert!(!go(&run, &probe).passed(), "and not inside it");
}

#[cfg(target_os = "macos")]
#[test]
fn a_target_writes_only_inside_the_run_the_cache_and_tmp_and_never_reads_forge() {
    let run = run();
    let outside = run
        .policy
        .run_dir
        .parent()
        .expect("root")
        .join("outside.txt");
    let wrote = go(&run, &format!("echo x > {}", outside.display()));
    assert!(!wrote.passed());
    assert!(!outside.exists());
    assert!(go(
        &run,
        "echo x > inside.txt && echo y > \"$TMP_TARGET\"/f 2>/dev/null || echo x > inside2.txt"
    )
    .passed());
    assert!(run.policy.run_dir.join("inside.txt").exists());
    let cache = run.policy.cache_dir.join("c");
    assert!(go(&run, &format!("echo x > {}", cache.display())).passed());
    let secret = run.policy.denied_reads[0].join("secret");
    assert!(!go(&run, &format!("cat {}", secret.display())).passed());
}

#[cfg(target_os = "macos")]
#[test]
fn a_target_past_its_timeout_is_killed_with_its_children() {
    let run = run();
    let result = run_target(
        &target("sleep 100 & echo $! > pid; wait"),
        Duration::from_secs(1),
        &run.policy,
        &env(),
    )
    .expect("ran");
    assert!(result.timed_out);
    assert_eq!(result.exit_code, None);
    assert!(result.duration < Duration::from_secs(10));
    let pid: i32 = std::fs::read_to_string(run.policy.run_dir.join("pid"))
        .expect("pid")
        .trim()
        .parse()
        .expect("number");
    let deadline = Instant::now() + Duration::from_secs(5);
    let gone = loop {
        let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
        if !alive || Instant::now() > deadline {
            break !alive;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(gone, "the grandchild sleep was killed with the group");
}

#[cfg(target_os = "macos")]
#[test]
fn the_target_sees_no_forge_variable() {
    let run = run();
    let resolved = |name: &str| std::env::var(name).ok();
    let env = environment(
        &["PATH".into(), "HOME".into()],
        &resolved,
        &[],
        &run.policy.cache_dir,
    );
    let result =
        run_target(&target("env"), Duration::from_secs(30), &run.policy, &env).expect("ran");
    assert!(result.passed());
    assert!(!result.stdout.contains("FORGE_SOCKET"), "{}", result.stdout);
    assert!(result.stdout.contains("FORGE_SANDBOX_CACHE="));
    assert!(!result
        .stdout
        .lines()
        .any(|line| line.starts_with("FORGE_") && !line.starts_with("FORGE_SANDBOX_CACHE=")));
}

#[test]
#[cfg(not(target_os = "macos"))]
fn elsewhere_sandbox_checks_are_unsupported() {
    let run = run();
    assert_eq!(
        run_target(&target("true"), Duration::from_secs(1), &run.policy, &env()).unwrap_err(),
        UNSUPPORTED
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_run_inside_a_denied_directory_still_works_and_its_siblings_stay_denied() {
    let mut run = run();
    let home = run.policy.denied_reads[0].clone();
    let inside = home.join("worktrees").join("w1");
    let cache = inside.join(".forge/review/sandbox/cache");
    std::fs::create_dir_all(&cache).expect("dir");
    std::fs::write(inside.join("pr.rs"), "x").expect("file");
    run.policy.run_dir = inside;
    run.policy.cache_dir = cache.clone();
    assert!(go(&run, "test -f pr.rs && echo ok > out").passed());
    assert!(go(&run, &format!("echo x > {}", cache.join("c").display())).passed());
    assert!(!go(&run, &format!("cat {}", home.join("secret").display())).passed());
}

#[test]
fn a_denied_directory_exempts_the_run_and_the_cache_it_contains() {
    let home = PathBuf::from("/forge");
    let policy = Policy {
        run_dir: home.join("worktrees/w1"),
        cache_dir: home.join("worktrees/w1/.forge/review/sandbox/cache"),
        tmp_dir: PathBuf::from("/tmp"),
        denied_reads: vec![home],
        network: false,
        socket: None,
    };
    assert!(profile(&policy).contains(
        "(deny file-read-data file-write* (require-all (subpath \"/forge\") \
         (require-not (require-any (subpath \"/forge/worktrees/w1\") \
         (subpath \"/forge/worktrees/w1/.forge/review/sandbox/cache\")))))\n"
    ));
}

/// `true` while `pid` is a child of this process that nobody has reaped; never reaps it.
#[cfg(target_os = "macos")]
fn unreaped_child(pid: i32) -> bool {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: `info` is valid, writable storage for one `siginfo_t` for the whole call.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid.unsigned_abs(),
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    rc == 0
}

#[cfg(target_os = "macos")]
#[test]
fn a_normal_exit_kills_what_it_left_running_and_then_reaps_its_leader() {
    let run = run();
    let result = go(&run, "echo $$ > leader; sleep 100 & echo $! > pid; exit 0");
    assert!(result.passed(), "{result:?}");
    assert!(
        result.duration < REAP_GRACE,
        "the leftover was killed, not waited out"
    );
    let read = |name: &str| -> i32 {
        std::fs::read_to_string(run.policy.run_dir.join(name))
            .expect("pid")
            .trim()
            .parse()
            .expect("number")
    };
    assert!(
        !unreaped_child(read("leader")),
        "run_target reaps its leader before it returns"
    );
    let sleeper = nix::unistd::Pid::from_raw(read("pid"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while nix::sys::signal::kill(sleeper, None).is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(nix::sys::signal::kill(sleeper, None).is_err());
}

#[test]
fn a_forge_variable_in_env_never_reaches_the_target() {
    let config = parse(
        "[env]\nFORGE_SOCKET = \"x\"\nFORGE_SESSION_ID = \"y\"\nLANG = \"C\"\n[[targets]]\nname = \"a\"\ncommand = \"b\"\n",
    )
    .expect("valid");
    let env = environment(
        &["PATH".into()],
        &|_| Some("/usr/bin".into()),
        &config.env,
        Path::new("/c"),
    );
    assert_eq!(
        env,
        [
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("LANG".to_owned(), "C".to_owned()),
            ("FORGE_SANDBOX_CACHE".to_owned(), "/c".to_owned()),
        ]
    );
}

/// Exits 0 when it can connect to the unix socket, 1 when it cannot.
fn connect(socket: &Path) -> String {
    format!(
        "/usr/bin/perl -MIO::Socket::UNIX -e 'IO::Socket::UNIX->new(Peer => shift) or exit 1' {}",
        socket.display()
    )
}

#[cfg(target_os = "macos")]
#[test]
fn with_the_network_on_the_forge_socket_stays_unreachable() {
    let mut run = run();
    let dir = tempfile::tempdir_in("/tmp").expect("socket dir");
    let socket = dir.path().canonicalize().expect("canonical").join("d.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("listen");
    let probe = connect(&socket);
    let outside = Command::new("/bin/sh")
        .arg("-c")
        .arg(&probe)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("nc");
    assert!(outside.success(), "the probe connects without the sandbox");
    run.policy.network = true;
    run.policy.socket = Some(socket.clone());
    assert!(
        !go(&run, &probe).passed(),
        "and not inside it, network on or not"
    );
    let other = dir
        .path()
        .canonicalize()
        .expect("canonical")
        .join("other.sock");
    let _other = std::os::unix::net::UnixListener::bind(&other).expect("listen");
    let reached = go(&run, &connect(&other));
    assert!(
        reached.passed(),
        "only Forge's socket is denied: {reached:?}"
    );
}

#[test]
fn a_checkout_with_only_the_earlier_file_is_told_to_rename_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(missing(dir.path()), "no .forge/review/checks.toml");
    std::fs::create_dir_all(dir.path().join(".forge/review")).expect("dir");
    std::fs::write(dir.path().join(".forge/review/sandbox.toml"), "").expect("file");
    assert_eq!(
        missing(dir.path()),
        "no .forge/review/checks.toml; rename .forge/review/sandbox.toml to it"
    );
}
