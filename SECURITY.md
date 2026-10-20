# Security policy

Please don't report security problems in public issues. Report them privately through
[GitHub's private vulnerability reporting](https://github.com/josephrich98/biocartoon-app/security/advisories/new),
or by email to josephrich98@gmail.com.

Useful things to include: what the problem is, which part it affects (desktop app, web app at
app.biocartoon.com, or the website), the version (shown under "About BioCartoon", or from
`biocartoon --version`), and steps to reproduce it.

Security fixes go into the newest release only.

Areas of particular interest:

- account sign-in and the session token the app keeps
- AI provider API keys you paste into the app
- the desktop app's local control socket (`control.json`, used by the CLI and MCP server)
- shared-library submissions and the files they carry
