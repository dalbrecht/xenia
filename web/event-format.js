// Presentation of the subset of runner events that is meaningful in a conversation.
// Unknown events remain in persisted output so future versions can understand them.
function formatRunnerEvents(raw) {
  const events = raw.split('\n').filter(Boolean).map(line => { try { return JSON.parse(line); } catch { return line; } });
  const hasPartial = events.some(event => event?.type === 'stream_event' && event.event?.delta?.type === 'text_delta');
  const codexDeltas = new Set(events.filter(event => event?.type === 'codex.delta').map(event => event.item_id));
  const liveDenials = events.filter(event => event?.type === 'system' && event.subtype === 'permission_denied');
  let text = '';
  for (const event of events) {
    if (typeof event === 'string') { text += event + '\n'; continue; }
    if (!event || typeof event !== 'object') continue;
    if (event.type === 'codex.delta') { text += event.text || ''; continue; }
    if (event.type === 'codex.message') { if (!codexDeltas.has(event.item_id)) text += event.text || ''; text += '\n'; continue; }
    if (event.type === 'claude.request_unsupported') { text += '\n[Claude request could not be handled. Review this turn.]\n'; continue; }
    if (event.type === 'codex.request_unsupported') { text += `\n[Codex request could not be handled: ${event.method || 'unknown request'}. Review this turn.]\n`; continue; }
    if (event.type === 'stream_event' && event.event?.delta?.type === 'text_delta') { text += event.event.delta.text || ''; continue; }
    if (event.type === 'item.started' && event.item?.type === 'command_execution') { text += `\n[Running: ${event.item.command || 'command'}]\n`; continue; }
    if (event.type === 'item.completed' && event.item?.type === 'agent_message') { text += (event.item.text || '') + '\n'; continue; }
    if (event.type === 'item.completed' && event.item?.type === 'command_execution' && event.item.exit_code != null && event.item.exit_code !== 0) {
      text += `\n[Command exited with status ${event.item.exit_code}]\n${String(event.item.aggregated_output || '').slice(-4000)}\n`; continue;
    }
    if (event.type === 'error' || event.type === 'turn.failed') {
      const message = event.message || event.error?.message || 'The runner reported an error.';
      text += `\n[Runner error: ${message}]\n`; continue;
    }
    if (event.type === 'system' && event.subtype === 'permission_denied') {
      const name = event.tool_name || event.tool?.name || 'a tool';
      text += `\n[Permission denied for ${name}. This action did not run.]\n`; continue;
    }
    if (event.type === 'result') {
      if (event.is_error) text += `\n[Runner error: ${event.result || 'The run failed.'}]\n`;
      const denials = Array.isArray(event.permission_denials) ? event.permission_denials : [];
      const liveNames = new Set(liveDenials.map(item => item.tool_name || item.tool?.name).filter(Boolean));
      const allShown = liveDenials.length >= denials.length && denials.every(item => item?.tool_name && liveNames.has(item.tool_name));
      if (denials.length && !allShown) {
        const names = [...new Set(denials.map(item => item?.tool_name).filter(name => typeof name === 'string'))];
        text += `\n[${denials.length} action${denials.length === 1 ? ' was' : 's were'} denied${names.length ? `: ${names.join(', ')}` : ''}. These actions did not run.]\n`;
      }
      for (const denial of denials) {
        if (!denial?.tool_input || typeof denial.tool_input !== 'object') continue;
        const input = typeof denial.tool_input.command === 'string' ? denial.tool_input.command : JSON.stringify(denial.tool_input);
        if (input) text += `\n[Denied action details: ${input.length > 2000 ? input.slice(0, 2000) + '…' : input}]\n`;
      }
      continue;
    }
    if (event.type === 'assistant') {
      for (const item of event.message?.content || []) {
        if (item.type === 'tool_use') text += `\n[Using ${item.name || 'a tool'}]\n`;
        else if (item.type === 'text' && !hasPartial) text += (item.text || '') + '\n';
      }
    }
  }
  return text.trim();
}
if (typeof module !== 'undefined' && module.exports) module.exports = { formatRunnerEvents };
if (typeof window !== 'undefined') window.formatRunnerEvents = formatRunnerEvents;
