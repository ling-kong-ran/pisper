export function pisperSandboxCoverage({ executionMode, capabilities } = {}) {
  if (executionMode === 'full-access') {
    return {
      overall: 'bypassed',
      entries: { bash: 'bypassed', fileTools: 'host-policy-checked', stdioMcp: 'unsandboxed' },
    }
  }
  if (executionMode === 'read-only') {
    return {
      overall: 'partial',
      entries: { bash: 'unavailable', fileTools: 'host-policy-checked', stdioMcp: 'unsandboxed' },
    }
  }
  return {
    overall: 'partial',
    entries: {
      bash: capabilities?.status || 'unavailable',
      fileTools: 'host-policy-checked',
      stdioMcp: 'unsandboxed',
    },
  }
}
