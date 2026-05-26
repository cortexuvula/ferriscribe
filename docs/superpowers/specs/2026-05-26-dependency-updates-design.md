# Dependency Updates Design Spec

**Date:** 2026-05-26  
**Status:** Approved  
**Priority:** Medium (Tech Debt)  
**Driver:** Security vulnerabilities

## Problem Statement

The application has 16 outdated npm dependencies, including 8 major version jumps. These outdated packages may contain known security vulnerabilities that need to be addressed.

**Current state:**
- 16 outdated packages total
- 8 major version updates (breaking changes)
- 8 minor/patch updates (safe)
- Frontend tests currently failing due to vite version mismatch

## Design: Security-First Dependency Update Strategy

### Approach Rationale

**Why security-first?**
- Primary driver is closing security vulnerabilities
- Prioritize updates by vulnerability severity (critical → high → medium)
- Apply safe minor/patch updates first for quick wins
- Then tackle major versions in security-priority order

**Alternative approaches considered:**
1. **Bottom-up dependency order** — builds from stable foundation but doesn't prioritize security risk
2. **Quick wins then major versions** — fast confidence-building but leaves security gaps longer

### Phase 1: Quick Wins (8 minor/patch updates)

Apply low-risk updates immediately:

| Package | Current | Latest | Type |
|---------|---------|--------|------|
| @tauri-apps/cli | 2.10.1 | 2.11.2 | minor |
| @tauri-apps/plugin-dialog | 2.7.0 | 2.7.1 | patch |
| @tauri-apps/plugin-opener | 2.5.3 | 2.5.4 | patch |
| svelte | 5.55.7 | 5.55.9 | patch |
| svelte-check | 4.4.6 | 4.4.8 | patch |

**Validation after applying:**
- `npm run check` (type checking)
- `npm run test:run` (unit tests)
- Manual smoke test: start dev server, verify basic functionality

**Estimated effort:** 30 minutes

### Phase 2: Security Research (8 major updates)

Before updating, research each package for:
- Known CVEs / security advisories
- Breaking changes in migration guides
- Dependency conflicts

**Packages to research:**

| Package | Current | Latest | Category |
|---------|---------|--------|----------|
| vite | 6.4.2 | 8.0.14 | Build tool (high security impact) |
| vitest | 2.1.9 | 4.1.7 | Test runner |
| @vitest/ui | 2.1.9 | 4.1.7 | Test UI |
| @tiptap/core | 2.27.2 | 3.23.6 | Editor (4 related packages) |
| @tiptap/extension-underline | 2.27.2 | 3.23.6 | Editor extension |
| @tiptap/pm | 2.27.2 | 3.23.6 | Editor plugin |
| @tiptap/starter-kit | 2.27.2 | 3.23.6 | Editor starter kit |
| @sveltejs/vite-plugin-svelte | 5.1.1 | 7.1.2 | Svelte integration |
| jsdom | 25.0.1 | 29.1.1 | DOM simulation |
| typescript | 5.9.3 | 6.0.3 | Type checker |
| tiptap-markdown | 0.8.10 | 0.9.0 | Markdown plugin |

**Research sources:**
- npm audit
- GitHub security advisories
- Package changelogs
- Migration guides

**Estimated effort:** 1 hour

### Phase 3: Prioritized Major Updates

Order based on research results:
1. **Critical security vulnerabilities** — update immediately
2. **High security vulnerabilities** — update next
3. **Medium security vulnerabilities** — update after
4. **No known vulnerabilities** — update last, by complexity

**For each major update:**
1. Read migration guide
2. Update package.json
3. Run `npm install`
4. Run `npm run check`
5. Run `npm run test:run`
6. Fix breaking changes
7. Commit
8. Move to next package

**Estimated effort:** 4-8 hours

## Execution Approach

### Git Strategy
- Use isolated git worktree: `.worktrees/dependency-updates`
- Branch name: `chore/dependency-updates-security`
- One commit per major update (easy to revert if issues)
- Final PR with all updates

### Risk Mitigation
- Each major update is a separate commit (easy to bisect)
- Full test suite after each update
- Manual smoke test before committing
- Rollback plan: revert individual commits if issues found

### Success Criteria
- [ ] All 16 packages updated to latest versions
- [ ] `npm run check` passes with no errors
- [ ] `npm run test:run` passes with all 31 tests passing
- [ ] `npm run build` succeeds
- [ ] Manual smoke test: dev server starts, basic features work
- [ ] No known security vulnerabilities in updated packages
- [ ] All breaking changes documented and resolved

## Dependencies and Constraints

### Constraints
- Must maintain compatibility with Rust/Tauri backend
- Cannot break existing functionality
- Must preserve HIPAA compliance (no new data collection)

### Dependencies
- Node.js version compatibility
- Tauri CLI version compatibility
- Vite plugin ecosystem compatibility

## Testing Strategy

### Automated Tests
- `npm run check` — TypeScript type checking
- `npm run test:run` — Vitest unit tests (31 tests)
- `npm run build` — Production build verification

### Manual Tests
- Dev server starts successfully
- Recording tab loads and functions
- SOAP generation works
- Settings panel accessible
- File import/export functions

## Rollout Plan

1. Create worktree and branch
2. Apply Phase 1 updates, test, commit
3. Research Phase 2 vulnerabilities
4. Apply Phase 3 updates in priority order
5. Final comprehensive testing
6. Create PR with detailed change log
7. Code review
8. Merge to master
9. Create release tag

## Success Metrics

- **Security:** Zero known critical/high vulnerabilities
- **Stability:** All tests pass, no regressions
- **Maintainability:** Easier future updates (no major version gaps)
- **Timeline:** Complete within 1-2 weeks

## Notes

- This is an aggressive timeline — assumes no major blockers
- If critical issues arise, may need to extend timeline
- Some breaking changes may require code refactoring
- Document all breaking changes and fixes in commit messages
