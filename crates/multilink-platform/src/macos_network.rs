//! An ephemeral authorized networking helper. The GUI never runs as root.
//! A root watchdog restores proxy/DNS settings when either process disappears.
pub use crate::macos_proxy::NetworkSnapshot;
use crate::macos_proxy::ProxyTransaction;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

const ROOT: &str = "/private/var/run/everty-multilink";
const MAX_MESSAGE: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
enum Request {
    StartCore { config: String },
    StopCore,
    PollCore,
    EnableProxy { port: u16 },
    RestoreProxy,
    Ping,
}
#[derive(Default, Serialize, Deserialize)]
struct Response {
    error: Option<String>,
    pid: Option<u32>,
    exit: Option<i32>,
}

fn send<T: Serialize>(stream: &mut impl Write, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(io::Error::other("Слишком большая команда сетевой службе"));
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}
fn receive<T: for<'a> Deserialize<'a>>(stream: &mut impl Read) -> io::Result<T> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_MESSAGE {
        return Err(io::Error::other("Слишком большой ответ сетевой службы"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let (mut uid, mut gid) = (0, 0);
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

struct Transport {
    stream: UnixStream,
    _launcher: Child,
    directory: PathBuf,
}
impl Drop for Transport {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        let _ = fs::remove_dir_all(&self.directory);
        // The helper and osascript exit after EOF; reap without blocking the GUI.
        let _ = self._launcher.try_wait();
    }
}
#[derive(Clone)]
pub struct NetworkSession(Arc<Mutex<Transport>>);
impl NetworkSession {
    pub fn acquire() -> io::Result<Self> {
        let executable = std::env::current_exe()?.canonicalize()?;
        if executable
            .file_name()
            .is_none_or(|name| name != "multilink-app")
        {
            return Err(io::Error::other(
                "Сетевую службу macOS нужно запускать из приложения Everty MultiLink.app",
            ));
        }
        static SESSION: OnceLock<Mutex<Option<Arc<Mutex<Transport>>>>> = OnceLock::new();
        let mut cached = SESSION
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| io::Error::other("Сетевая служба занята"))?;
        if let Some(inner) = cached.as_ref() {
            let session = Self(inner.clone());
            if session.request(Request::Ping).is_ok() {
                return Ok(session);
            }
            *cached = None;
        }
        let directory = Path::new("/private/tmp").join(format!("ev-net-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let socket = directory.join("control.sock");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let token = uuid::Uuid::new_v4().to_string();
        let command = format!(
            "{} --macos-network-helper {} {} {}",
            shell_quote(&executable.to_string_lossy()),
            shell_quote(&socket.to_string_lossy()),
            shell_quote(&token),
            unsafe { libc::getuid() }
        );
        let script = "on run argv\ndo shell script (item 1 of argv) with administrator privileges with prompt \"Everty MultiLink: разрешить VPN, маршрутизацию и восстановление сетевых настроек\"\nend run";
        let mut launcher = Command::new("/usr/bin/osascript")
            .args(["-e", script, &command])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(120);
        let connection = loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // Darwin can inherit O_NONBLOCK from the listening socket.
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
                    if peer_uid(&stream)? != 0 {
                        continue;
                    }
                    let hello: String = receive(&mut stream)?;
                    if hello != token {
                        continue;
                    }
                    stream.set_read_timeout(Some(Duration::from_secs(20)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
                    break Ok(stream);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => break Err(error),
            }
            if launcher.try_wait()?.is_some() {
                break Err(io::Error::other(
                    "Доступ к сетевой службе не получен. Подтвердите системный запрос macOS",
                ));
            }
            if Instant::now() >= deadline {
                let _ = launcher.kill();
                break Err(io::Error::other(
                    "Истекло время подтверждения доступа к VPN",
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                let _ = fs::remove_dir_all(directory);
                return Err(error);
            }
        };
        let session = Self(Arc::new(Mutex::new(Transport {
            stream,
            _launcher: launcher,
            directory,
        })));
        session.request(Request::Ping)?;
        *cached = Some(session.0.clone());
        Ok(session)
    }
    fn request(&self, request: Request) -> io::Result<Response> {
        let mut transport = self
            .0
            .lock()
            .map_err(|_| io::Error::other("Сетевая служба недоступна"))?;
        let exchange: io::Result<Response> = (|| {
            send(&mut transport.stream, &request)?;
            receive(&mut transport.stream)
        })();
        let response = match exchange {
            Ok(response) => response,
            Err(error) => {
                let _ = transport.stream.shutdown(std::net::Shutdown::Both);
                return Err(error);
            }
        };
        match response.error.as_ref() {
            Some(error) => Err(io::Error::other(error.clone())),
            None => Ok(response),
        }
    }
    pub fn start_core(config: &str) -> io::Result<Self> {
        let session = Self::acquire()?;
        if let Err(error) = session.request(Request::StartCore {
            config: config.into(),
        }) {
            let _ = session.stop_core();
            return Err(error);
        }
        Ok(session)
    }
    pub fn stop_core(&self) -> io::Result<()> {
        self.request(Request::StopCore).map(|_| ())
    }
    pub fn core_exit(&self) -> io::Result<Option<i32>> {
        self.request(Request::PollCore).map(|r| r.exit)
    }
    pub fn enable_proxy(&self, port: u16) -> io::Result<()> {
        self.request(Request::EnableProxy { port }).map(|_| ())
    }
    pub fn restore_proxy(&self) -> io::Result<()> {
        self.request(Request::RestoreProxy).map(|_| ())
    }
    /// Integration check only: terminate this helper without running its Drop
    /// handlers, so the independent watchdog must restore the network itself.
    pub fn abort_own_helper_for_check(&self) -> io::Result<()> {
        let pid = self
            .request(Request::Ping)?
            .pid
            .ok_or_else(|| io::Error::other("Нет PID сетевой службы"))?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let command = format!("if [ \"$(/bin/ps -p {pid} -o comm=)\" = {} ]; then /bin/kill -KILL {pid}; else exit 1; fi", shell_quote(&executable.to_string_lossy()));
        let script = "on run argv\ndo shell script (item 1 of argv) with administrator privileges with prompt \"Everty MultiLink: проверка восстановления сети после аварии сетевой службы\"\nend run";
        let status = Command::new("/usr/bin/osascript")
            .args(["-e", script, &command])
            .status()?;
        if !status.success() {
            return Err(io::Error::other(
                "Аварийная проверка сетевой службы отменена",
            ));
        }
        Ok(())
    }
}

pub struct SystemProxyGuard {
    session: NetworkSession,
    active: bool,
}
impl SystemProxyGuard {
    pub fn enable(port: u16) -> io::Result<Self> {
        let session = NetworkSession::acquire()?;
        let mut guard = Self {
            session,
            active: true,
        };
        if let Err(error) = guard.session.enable_proxy(port) {
            let _ = guard.restore();
            return Err(error);
        }
        Ok(guard)
    }
    pub fn restore(&mut self) -> io::Result<()> {
        if self.active {
            self.session.restore_proxy()?;
            self.active = false;
        }
        Ok(())
    }
}
impl Drop for SystemProxyGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn root_lock(wait: bool) -> io::Result<File> {
    fs::create_dir_all(ROOT)?;
    let metadata = fs::symlink_metadata(ROOT)?;
    if metadata.uid() != 0 || !metadata.is_dir() {
        return Err(io::Error::other("Некорректная папка сетевой службы"));
    }
    fs::set_permissions(ROOT, fs::Permissions::from_mode(0o700))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(Path::new(ROOT).join("session.lock"))?;
    let operation = libc::LOCK_EX | if wait { 0 } else { libc::LOCK_NB };
    if unsafe { libc::flock(lock.as_raw_fd(), operation) } != 0 {
        return Err(io::Error::other(
            "Сетевая служба уже используется другим экземпляром MultiLink",
        ));
    }
    Ok(lock)
}
fn stop_pid(directory: &Path) {
    let Ok(pid) = fs::read_to_string(directory.join("core.pid"))
        .unwrap_or_default()
        .parse::<i32>()
    else {
        return;
    };
    if pid > 1 {
        watch_child(pid, &directory.join("mihomo"), &mut io::empty());
    }
}

pub(crate) fn process_path(pid: i32) -> Option<PathBuf> {
    let mut path = [0u8; 4096];
    if unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) } <= 0 {
        return None;
    }
    std::ffi::CStr::from_bytes_until_nul(&path)
        .ok()
        .map(|s| PathBuf::from(s.to_string_lossy().as_ref()))
}
fn watch_child(pid: i32, expected: &Path, input: &mut impl Read) {
    let _ = io::copy(input, &mut io::sink());
    if process_path(pid).as_deref() != Some(expected) {
        return;
    }
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && process_path(pid).as_deref() == Some(expected) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if process_path(pid).as_deref() == Some(expected) {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
}
fn recover_directory(directory: &Path) -> io::Result<()> {
    ProxyTransaction::recover(&directory.join("proxy.json"))?;
    stop_pid(directory);
    ProxyTransaction::recover(&directory.join("dns.json"))?;
    fs::remove_dir_all(directory)
}

struct RootSession {
    directory: PathBuf,
    core: Option<Child>,
    proxy: Option<ProxyTransaction>,
    dns: Option<ProxyTransaction>,
    _watchdog: Child,
}
impl RootSession {
    fn new() -> io::Result<Self> {
        for entry in fs::read_dir(ROOT)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                recover_directory(&entry.path())?;
            }
        }
        let directory = Path::new(ROOT).join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let watchdog = Command::new(std::env::current_exe()?)
            .arg("--macos-network-watchdog")
            .arg(&directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            directory,
            core: None,
            proxy: None,
            dns: None,
            _watchdog: watchdog,
        })
    }
    fn stop_core(&mut self) -> io::Result<()> {
        if let Some(mut core) = self.core.take() {
            if core.try_wait()?.is_none() {
                unsafe {
                    libc::kill(core.id() as i32, libc::SIGTERM);
                }
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline && core.try_wait()?.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            }
            if core.try_wait()?.is_none() {
                let _ = core.kill();
            }
            let _ = core.wait();
            let _ = fs::remove_file(self.directory.join("core.pid"));
        }
        if let Some(dns) = &self.dns {
            dns.restore()?;
        }
        self.dns = None;
        Ok(())
    }
    fn request(&mut self, request: Request) -> io::Result<Response> {
        match request {
            Request::Ping => {
                return Ok(Response {
                    pid: Some(std::process::id()),
                    ..Default::default()
                })
            }
            Request::StartCore { config } => {
                if self.core.is_some() {
                    return Err(io::Error::other("Ядро уже запущено"));
                }
                let mut config: serde_json::Value =
                    serde_yaml::from_str(&config).map_err(io::Error::other)?;
                let bundled = std::env::current_exe()?.with_file_name("mihomo");
                if !bundled.is_file() {
                    return Err(io::Error::other(
                        "Не найдено встроенное ядро Mihomo. Запустите приложение из .app",
                    ));
                }
                // Freeze the executable and configuration in a root-owned session.
                fs::copy(bundled, self.directory.join("mihomo"))?;
                fs::set_permissions(
                    self.directory.join("mihomo"),
                    fs::Permissions::from_mode(0o500),
                )?;
                if let Some(provider) = config
                    .get_mut("rule-providers")
                    .and_then(|p| p.get_mut("RU-SITES"))
                {
                    fs::write(
                        self.directory.join("category-ru.mrs"),
                        include_bytes!("../../../assets/category-ru.mrs"),
                    )?;
                    provider["path"] = serde_json::json!(self.directory.join("category-ru.mrs"));
                }
                let tun = config["tun"]["enable"].as_bool().unwrap_or(false);
                fs::write(
                    self.directory.join("config.yaml"),
                    serde_yaml::to_string(&config).map_err(io::Error::other)?,
                )?;
                let log = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .mode(0o600)
                    .open(self.directory.join("core.log"))?;
                let core = Command::new(self.directory.join("mihomo"))
                    .arg("-d")
                    .arg(&self.directory)
                    .arg("-f")
                    .arg(self.directory.join("config.yaml"))
                    .stdin(Stdio::null())
                    .stdout(Stdio::from(log.try_clone()?))
                    .stderr(Stdio::from(log))
                    .spawn()?;
                let pid = core.id();
                self.core = Some(core);
                fs::write(self.directory.join("core.pid"), pid.to_string())?;
                if tun {
                    self.dns = Some(ProxyTransaction::prepare_dns(
                        &self.directory.join("dns.json"),
                    )?);
                    self.dns.as_mut().unwrap().apply()?;
                }
                return Ok(Response {
                    pid: self.core.as_ref().map(Child::id),
                    ..Default::default()
                });
            }
            Request::StopCore => self.stop_core()?,
            Request::PollCore => {
                let exit = match &mut self.core {
                    Some(core) => core.try_wait()?.map(|status| status.code().unwrap_or(1)),
                    None => Some(0),
                };
                return Ok(Response {
                    exit,
                    ..Default::default()
                });
            }
            Request::EnableProxy { port } => {
                if self.proxy.is_some() {
                    return Err(io::Error::other("Системный прокси уже включён"));
                }
                self.proxy = Some(ProxyTransaction::prepare(
                    port,
                    &self.directory.join("proxy.json"),
                )?);
                self.proxy.as_mut().unwrap().apply()?;
            }
            Request::RestoreProxy => {
                if let Some(proxy) = &self.proxy {
                    proxy.restore()?;
                }
                self.proxy = None;
            }
        }
        Ok(Response::default())
    }
}
impl Drop for RootSession {
    fn drop(&mut self) {
        if let Some(proxy) = &self.proxy {
            let _ = proxy.restore();
        }
        let _ = self.stop_core();
        // Watchdog independently retries any restoration that failed here.
    }
}

fn run_helper(socket: &Path, token: &str, uid: u32) -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::other(
            "Сетевой службе нужны права администратора",
        ));
    }
    let mut stream = UnixStream::connect(socket)?;
    if peer_uid(&stream)? != uid {
        return Err(io::Error::other(
            "Неверный владелец управляющего соединения",
        ));
    }
    send(&mut stream, &token)?;
    if !matches!(receive::<Request>(&mut stream)?, Request::Ping) {
        return Err(io::Error::other(
            "Не выполнено подтверждение сетевой службы",
        ));
    }
    let initialized = (|| -> io::Result<_> {
        let lock = root_lock(false)?;
        let session = RootSession::new()?;
        Ok((lock, session))
    })();
    let (_lock, mut session) = match initialized {
        Ok(initialized) => initialized,
        Err(error) => {
            send(
                &mut stream,
                &Response {
                    error: Some(error.to_string()),
                    ..Default::default()
                },
            )?;
            return Ok(());
        }
    };
    send(&mut stream, &Response::default())?;
    while let Ok(request) = receive::<Request>(&mut stream) {
        let response = session.request(request).unwrap_or_else(|e| Response {
            error: Some(e.to_string()),
            ..Default::default()
        });
        if send(&mut stream, &response).is_err() {
            break;
        }
    }
    Ok(())
}

pub fn handle_helper_args() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--macos-child-watchdog") {
        if unsafe { libc::geteuid() } != 0 && args.len() == 4 {
            if let Some(pid) = args[2]
                .to_str()
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|pid| *pid > 1)
            {
                watch_child(pid, Path::new(&args[3]), &mut io::stdin());
            }
        }
        return true;
    }
    if args.get(1).is_some_and(|a| a == "--macos-network-helper") {
        let result = (|| {
            if args.len() != 5 {
                return Err(io::Error::other("Неверные аргументы сетевой службы"));
            }
            let uid = args[4]
                .to_str()
                .and_then(|a| a.parse().ok())
                .ok_or_else(|| io::Error::other("Неверный UID"))?;
            run_helper(Path::new(&args[2]), &args[3].to_string_lossy(), uid)
        })();
        if let Err(error) = result {
            eprintln!("{error}");
        }
        return true;
    }
    if args.get(1).is_some_and(|a| a == "--macos-network-watchdog") {
        if unsafe { libc::geteuid() } == 0 && args.len() == 3 {
            let directory = Path::new(&args[2]);
            if directory.parent() == Some(Path::new(ROOT)) {
                let _ = io::copy(&mut io::stdin(), &mut io::sink());
                if let Ok(_lock) = root_lock(true) {
                    let _ = recover_directory(directory);
                }
            }
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framed_protocol_handles_config_newlines_and_rejects_oversized_messages() {
        let mut bytes = Vec::new();
        send(
            &mut bytes,
            &Request::StartCore {
                config: "rules:\n  - MATCH,DIRECT\n".into(),
            },
        )
        .unwrap();
        assert!(
            matches!(receive::<Request>(&mut bytes.as_slice()).unwrap(), Request::StartCore { config } if config.contains("MATCH,DIRECT"))
        );
        assert!(receive::<Request>(&mut u32::MAX.to_be_bytes().as_slice()).is_err());
    }
    #[test]
    fn shell_arguments_remain_single_arguments() {
        assert_eq!(
            shell_quote("/Applications/A B.app/helper"),
            "'/Applications/A B.app/helper'"
        );
        assert_eq!(shell_quote("a'b$(whoami)"), "'a'\\''b$(whoami)'");
    }
    #[test]
    fn unix_channel_reports_actual_peer_identity() {
        let (a, b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), unsafe { libc::geteuid() });
        drop(b);
    }
    #[test]
    fn local_core_watchdog_stops_only_the_expected_child() {
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        let expected = process_path(pid).unwrap();
        watch_child(pid, Path::new("/not-the-same-executable"), &mut io::empty());
        assert!(child.try_wait().unwrap().is_none());
        watch_child(pid, &expected, &mut io::empty());
        assert!(!child.wait().unwrap().success());
    }
}
