/// Guide for setting up SSH over port 443 for GitHub
/// This is an alternative to HTTPS proxy for git operations

pub fn print_ssh_guide() {
    println!(
        r#"
╔══════════════════════════════════════════════════════════════════╗
║         SSH over 443 — GitHub Access Guide                      ║
╠══════════════════════════════════════════════════════════════════╣
║                                                                  ║
║  GitHub supports SSH connections on port 443, which may          ║
║  bypass some network restrictions.                               ║
║                                                                  ║
║  Setup steps:                                                    ║
║                                                                  ║
║  1. Edit your SSH config (~/.ssh/config):                        ║
║                                                                  ║
║     Host github.com                                              ║
║         Hostname ssh.github.com                                  ║
║         Port 443                                                  ║
║         User git                                                  ║
║         IdentityFile ~/.ssh/id_ed25519                           ║
║                                                                  ║
║  2. Test the connection:                                         ║
║                                                                  ║
║     $ ssh -T git@github.com                                      ║
║     Hi username! You've successfully authenticated...            ║
║                                                                  ║
║  3. Use SSH URLs for git operations:                             ║
║                                                                  ║
║     $ git clone git@github.com:user/repo.git                     ║
║                                                                  ║
║  Note: This bypasses HTTPS proxy entirely and uses SSH           ║
║  directly on port 443.                                           ║
║                                                                  ║
╚══════════════════════════════════════════════════════════════════╝
"#
    );
}
