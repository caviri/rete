# Expandable navigation and installation selector

The reader requested sidebar expanders and an installation selector inspired by
PyTorch. Keep the existing documentation design and the 15 primary destinations.
Use native disclosure elements for the four sections and a curated set of hub
children, with the current path open. Keep application experiments in their
gallery rather than restoring the old catalogue in the sidebar.

Add an Install page linked from Quickstart and Overview. Offer operating system,
interface and compatible package choices, verified against actual published
packages. Show copyable commands and prerequisites, separate CLI installation
from language libraries, and preserve useful instructions without JavaScript.
Published versions are explicit; the current source tree is not a release feed.

Implementation:

- [x] Add curated navigation children, native keyboard-accessible disclosures,
  current-path expansion and the same behavior in the mobile drawer.
- [x] Add the installation widget as a separate docgen module and concise page.
- [x] Check commands against package metadata and verify the selection/copy
  behavior, mobile layout and JavaScript-disabled fallback.
- [ ] Regenerate documentation and social cards, run docgen and documentation
  release checks and independently review. Deliver as a follow-up PR.

Work in the existing isolated worktree on `docs/navigation-install`, based on
`d1d8352d`. Root owns navigation/integration; the installer agent owns only its
module and page. No engine or application runtime changes.
