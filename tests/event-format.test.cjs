const { test } = require('node:test');
const assert = require('node:assert/strict');
const { formatRunnerEvents } = require('../web/event-format.js');
const lines = (...values) => values.map(value => JSON.stringify(value)).join('\n');

test('shows Claude permission denials even when the overall run succeeds', () => {
  const output = formatRunnerEvents(lines(
    {type:'system',subtype:'permission_denied',tool_name:'Bash'},
    {type:'result',subtype:'success',is_error:false,permission_denials:[{tool_name:'Bash'}]}
  ));
  assert.equal(output, '[Permission denied for Bash. This action did not run.]');
  const resultOnly = formatRunnerEvents(lines({type:'result',is_error:false,permission_denials:[{tool_name:'Bash'}]}));
  assert.match(resultOnly, /1 action was denied: Bash/);
  assert.match(resultOnly, /did not run/);
});

test('preserves partial text without duplicating the completed assistant message', () => {
  const output = formatRunnerEvents(lines(
    {type:'stream_event',event:{delta:{type:'text_delta',text:'Hello'}}},
    {type:'stream_event',event:{delta:{type:'text_delta',text:' world'}}},
    {type:'assistant',message:{content:[{type:'text',text:'Hello world'}]}},
    {type:'result',is_error:false,result:'Hello world',permission_denials:[]}
  ));
  assert.equal(output, 'Hello world');
});

test('renders Codex app-server deltas once and falls back to complete messages', () => {
  const output = formatRunnerEvents(lines(
    {type:'codex.delta',item_id:'one',text:'Hello'},
    {type:'codex.delta',item_id:'one',text:' world'},
    {type:'codex.message',item_id:'one',text:'Hello world'},
    {type:'codex.message',item_id:'two',text:'Second message'}
  ));
  assert.equal(output, 'Hello world\nSecond message');
});

test('makes unsupported Codex requests visible', () => {
  assert.match(formatRunnerEvents(lines({type:'codex.request_unsupported',method:'item/tool/requestUserInput'})), /could not be handled/);
});

test('shows the input of a denied action for review', () => {
  const output = formatRunnerEvents(lines({type:'result',is_error:false,permission_denials:[{tool_name:'Bash',tool_input:{command:'printf synthetic'}}]}));
  assert.match(output, /Denied action details: printf synthetic/);
});

test('surfaces Codex errors and unsuccessful commands', () => {
  const output = formatRunnerEvents(lines(
    {type:'item.completed',item:{type:'command_execution',exit_code:2,aggregated_output:'synthetic failure'}},
    {type:'turn.failed',error:{message:'synthetic runner error'}}
  ));
  assert.match(output, /status 2/);
  assert.match(output, /synthetic failure/);
  assert.match(output, /synthetic runner error/);
});

test('handles malformed and unknown events without hiding plain diagnostics', () => {
  assert.equal(formatRunnerEvents('plain diagnostic\nnull\n{"type":"unknown"}\n'), 'plain diagnostic');
});
