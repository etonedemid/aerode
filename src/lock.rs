//! Lock screen: the compositor hides everything and hands the screen to `aerode-lockui`, which shows
//! Win7's locked screen and has PAM check the password through xsecurelock's `authproto_pam` helper.
//! The compositor only unlocks when that program exits 0; if it dies, it is started again, still locked.
use crate::{util, wmipc};

const AUTHPROTO: [&str; 3] = ["/usr/libexec/xsecurelock/authproto_pam", "/usr/lib/xsecurelock/authproto_pam", "/usr/local/libexec/xsecurelock/authproto_pam"];

/// Lock the screen. Like Win7's Explorer (WTS_SESSION_LOCK / UNLOCK) this plays Windows Logoff
/// as the screen locks - unless `lock_sound` is false (locking for sleep) - and Windows Logon
/// once it has been unlocked.
pub fn lock(lock_sound: bool) {
    // a lock nobody can open is worse than no lock: refuse and say why
    if !util::which("aerode-lockui") || !AUTHPROTO.iter().any(|p| std::path::Path::new(p).exists()) {
        util::error("The computer could not be locked.", "Install xsecurelock: the lock screen checks passwords with its authproto_pam helper.");
        return;
    }
    if lock_sound { crate::sounds::play_with(crate::sounds::Event::Logoff, crate::sounds::Policy::Yield); }
    wmipc::send(wmipc::Req::Lock { cmd: "aerode-lockui".into() });
    loop {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let locked = wmipc::call(&wmipc::Req::Locked).and_then(|v| v.get("locked")?.as_bool());
        if locked != Some(true) { break; }
    }
    crate::sounds::play_sync(crate::sounds::Event::Logon);
}
