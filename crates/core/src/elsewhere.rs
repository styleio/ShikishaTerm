//! Doing things on a machine that is not this one.
//!
//! There are two kinds, and they have almost nothing in common underneath. A
//! server reached over SSH is already there, has an address, and speaks one
//! connection that carries both a terminal and files. A sandbox in the cloud
//! does not exist until it is asked for, has no address to speak of, and
//! answers three different HTTP services.
//!
//! Nothing above this has any business knowing that. A file panel wants a
//! listing, a git command wants an exit code, and either machine can give one.
//! So the difference stops here: [`Elsewhere`] says which, and the two
//! functions below say what, and everything else names a machine and asks.
//!
//! **Naming one costs nothing.** A sandbox is made by the first call that
//! actually needs it, not by building the value -- which is what lets a screen
//! that merely draws a file panel hold one without renting a machine to do it.
//!
//! What is deliberately *not* here is the terminal. A terminal is handed out
//! once and then belongs to the tab that holds it -- it is a thing, not a
//! question -- so [`crate::ssh::shell`] and [`crate::e2b::shell`] are called
//! where a tab is built, and both return the same [`portable_pty::MasterPty`].

use anyhow::Result;

/// A machine that is not this one, named rather than connected to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elsewhere {
    /// One that is already there, reached over SSH
    Ssh(crate::ssh::Spec),
    /// One that is made when something is finally wanted of it
    Cloud(crate::config::HostSpec),
}

impl Elsewhere {
    /// The machine a settings entry names.
    ///
    /// Fails only for a written-down address that cannot be read as one. A
    /// sandbox cannot fail here because nothing has been asked of it yet
    pub fn of(host: &crate::config::HostSpec) -> Result<Self> {
        match host.is_made() {
            true => Ok(Self::Cloud(host.clone())),
            false => Ok(Self::Ssh(crate::config::host_spec(host)?)),
        }
    }

    /// What to call it in front of a person.
    ///
    /// Never a credential and never a token: an address is the writing on an
    /// envelope, and a sandbox has no address -- what it has is the name
    /// somebody gave it in the settings, which is the name they will recognise
    pub fn address(&self) -> String {
        match self {
            Self::Ssh(spec) => spec.address(),
            Self::Cloud(host) => host.name.clone(),
        }
    }

    /// Who the work is done as, where that is a thing a person chose. A
    /// sandbox hands out one account and it is not anybody's choice
    pub fn user(&self) -> Option<&str> {
        match self {
            Self::Ssh(spec) => Some(&spec.user),
            Self::Cloud(_) => None,
        }
    }
}

/// Do something with files on it.
///
/// Blocks until the far end answers, like every other file call in this
/// program. Whoever calls it decides how long they are willing to wait --
/// which, for a sandbox that has to be built first, has to be long enough to
/// build one
pub fn files(
    at: &Elsewhere,
    job: crate::ssh::FileJob,
    wait_ms: u64,
) -> Result<crate::ssh::FileAnswer> {
    match at {
        Elsewhere::Ssh(spec) => crate::ssh::files(spec, job, wait_ms),
        Elsewhere::Cloud(host) => {
            crate::e2b::files(&crate::e2b::sandbox_for(host, None)?, job, wait_ms)
        }
    }
}

/// Run one command on it, and wait for the answer.
pub fn exec(at: &Elsewhere, command: &str, wait_ms: u64) -> Result<crate::ssh::Ran> {
    match at {
        Elsewhere::Ssh(spec) => crate::ssh::exec(spec, command, wait_ms),
        // The far end keeps its own deadline on this call, so the number is
        // not passed on -- it would be a second one meaning something else
        Elsewhere::Cloud(host) => {
            crate::e2b::exec(&crate::e2b::sandbox_for(host, None)?, command, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud() -> crate::config::HostSpec {
        crate::config::HostSpec {
            name: "cloud".into(),
            // Left over from somebody editing the entry before changing its
            // kind. It must not decide anything
            at: "ssh://someone@leftover:22".into(),
            kind: Some("e2b".into()),
            ..Default::default()
        }
    }

    fn server() -> crate::config::HostSpec {
        crate::config::HostSpec {
            name: "server".into(),
            at: "ssh://someone@example.com:2222".into(),
            ..Default::default()
        }
    }

    /// Which kind of machine an entry means is read from the entry, not
    /// guessed from whether it happens to have an address written in it
    #[test]
    fn the_entry_says_which_kind_it_is() {
        assert!(matches!(Elsewhere::of(&cloud()).unwrap(), Elsewhere::Cloud(_)));
        match Elsewhere::of(&server()).expect("住所があるのに組めない") {
            Elsewhere::Ssh(spec) => {
                assert_eq!(spec.host, "example.com");
                assert_eq!(spec.port, 2222);
            }
            Elsewhere::Cloud(_) => panic!("SSH の場所をサンドボックスにした"),
        }
    }

    /// Naming a machine must not rent one. A file panel on screen holds one of
    /// these for as long as it is drawn, and a sandbox costs money per minute
    /// -- so building the value has to be arithmetic, not a purchase
    #[test]
    fn naming_a_machine_does_not_make_one() {
        // No key is set in a test run, so anything that tried to make a
        // sandbox here would fail rather than succeed quietly
        let named = Elsewhere::of(&cloud()).expect("名前を付けるだけで失敗した");
        assert_eq!(named, Elsewhere::Cloud(cloud()));
        assert_eq!(named.address(), "cloud");
    }

    /// What a person is shown about a machine is where it is, and nothing else
    /// that happens to be in the settings beside it
    #[test]
    fn a_machine_is_named_by_where_it_is() {
        let at = Elsewhere::Ssh(crate::ssh::Spec {
            host: "example.com".into(),
            port: 2222,
            user: "someone".into(),
            password_key: Some("ssh/host/server/password".into()),
            ..Default::default()
        });
        let said = at.address();
        assert!(said.contains("example.com"), "{said}");
        assert!(!said.contains("password"), "秘密の名前が出ている: {said}");
        assert_eq!(at.user(), Some("someone"));

        // A sandbox has one account and nobody chose it, so there is no name
        // to put in front of the address on screen
        assert_eq!(Elsewhere::Cloud(cloud()).user(), None);
    }
}
