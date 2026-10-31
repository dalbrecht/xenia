const $ = selector => document.querySelector(selector);
const escapeHtml = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
let state = { projects: [], nodes: [], jobs: [], now: 0 };
let view = 'conversations', selected = null, projectFilter = '', nodeFilter = '';
let token = localStorage.getItem('xenia-device-token') || sessionStorage.getItem('xenia-token') || '';
function takePairingCode() {
  const code = new URLSearchParams(location.hash.slice(1)).get('pair');
  if (code) history.replaceState(null, '', location.pathname + location.search);
  return code;
}
const pairingCode = takePairingCode();
let authGeneration = 0;
let outputCache = {};
let approvalCache = {};
let approvalGeneration = 0;
let followTarget = null, followDraft = '';
let questionDrafts = {};
let deferredRender = false;
function renderWhenNotEditing() {
  if (document.activeElement?.closest('[data-follow-form], [data-question-form]')) { deferredRender = true; return; }
  deferredRender = false; render();
}
document.addEventListener('focusout', () => setTimeout(() => { if (deferredRender) renderWhenNotEditing(); }, 0));

async function api(path, body) {
  const generation = authGeneration;
  const response = await fetch('/api/' + path, {
    method: body ? 'POST' : 'GET',
    headers: { Authorization: 'Bearer ' + token, ...(body ? { 'Content-Type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined
  }).catch(error => { if (generation !== authGeneration) throw Error('Session ended.'); throw error; });
  if (generation !== authGeneration) throw Error('Session ended.');
  if (response.status === 401) {
    localStorage.removeItem('xenia-device-token');
    if (!$('#auth').open) $('#auth').showModal();
    throw Error('Enter a valid controller token.');
  }
  if (!response.ok) {
    let error;
    try { error = (await response.json()).error; } catch {}
    throw Error(error || 'Request failed');
  }
  const result = await response.json();
  if (generation !== authGeneration) throw Error('Session ended.');
  return result;
}

const status = value => `<span class="status ${escapeHtml(value)}"><i></i>${escapeHtml(value.replaceAll('_', ' '))}</span>`;
const shorten = value => escapeHtml(value.length > 90 ? value.slice(0, 90) + '…' : value);
function groups() {
  const byId = new Map(state.jobs.map(job => [job.id, job]));
  const grouped = new Map();
  for (const job of state.jobs) {
    let root = job, visited = new Set();
    while (root.parent_id && byId.has(root.parent_id) && !visited.has(root.id)) {
      visited.add(root.id); root = byId.get(root.parent_id);
    }
    if (!grouped.has(root.id)) grouped.set(root.id, []);
    grouped.get(root.id).push(job);
  }
  return [...grouped].map(([id, jobs]) => ({ id, jobs: jobs.reverse() }));
}
function actions(job) {
  const terminal = ['succeeded', 'failed', 'canceled', 'interrupted'].includes(job.state);
  const cleanupBlocked = ['queued', 'running', 'unknown', 'succeeded'].includes(job.cleanup_state);
  return `<div class="actions job-actions">
    ${job.cleanup_state === 'unknown' ? `<button data-cleanup-resolve="${escapeHtml(job.id)}" class="secondary">Resolve cleanup</button>` : ''}
    ${job.state === 'unknown' ? `<button data-resolve="${escapeHtml(job.id)}" class="secondary">Resolve interrupted work</button>` : ''}
    ${['queued', 'running'].includes(job.state) ? `<button data-cancel="${escapeHtml(job.id)}" class="secondary">Cancel work</button>` : ''}
    ${job.worktree && terminal && !cleanupBlocked ? `<button data-cleanup="${escapeHtml(job.id)}" class="secondary">Remove worktree</button>` : ''}
    ${job.session_id && terminal && !cleanupBlocked && !state.jobs.some(other => other.parent_id === job.id) ? `<button data-follow="${escapeHtml(job.id)}" class="primary">Follow up</button>` : ''}
  </div>`;
}
function queueReason(job) {
  if (job.state !== 'queued') return '';
  const candidates = job.node_id ? state.nodes.filter(n => n.id === job.node_id) : state.nodes;
  if (!candidates.some(n => state.now - n.last_seen < 15)) return 'Waiting for an assigned or eligible node to connect.';
  const eligible = candidates.filter(n => state.now - n.last_seen < 15 && JSON.parse(n.runners).includes(job.runner) && JSON.parse(n.eligible).includes(job.project_id));
  if (!eligible.length) return 'Waiting for a connected node with the runner and repository.';
  return 'Waiting for available node capacity.';
}
function details(jobs) {
  const last = jobs[jobs.length - 1];
  return `<div class="detail"><div class="detail-meta"><span>Node: ${escapeHtml(state.nodes.find(n => n.id === last.node_id)?.name || 'Automatic')}</span><span>Branch: ${escapeHtml(last.branch || 'Pending')}</span></div>
  ${last.worktree ? `<p class="path">${escapeHtml(last.worktree)}</p>` : ''}
  ${queueReason(last) ? `<p class="path">${escapeHtml(queueReason(last))}</p>` : ''}
  ${last.cleanup_state ? `<p class="path">Cleanup: ${escapeHtml(last.cleanup_state)} ${escapeHtml(last.cleanup_error || '')}</p>` : ''}
  ${jobs.map(job => { const output = outputCache[job.id]; return `<div class="message"><strong>You</strong><p>${escapeHtml(job.prompt)}</p><strong>${escapeHtml(job.runner)}</strong>${job.error ? `<p class="error">${escapeHtml(job.error)}</p>` : ''}${output?.history?.length ? '<p class="path">Viewing earlier output; live updates are shown on the latest page.</p>' : ''}<pre>${escapeHtml(formatRunnerEvents(output?.text || '') || 'Waiting for agent output…')}</pre>${output?.has_more || output?.history?.length ? `<div class="actions"><button class="secondary" data-output-older="${escapeHtml(job.id)}" ${output.has_more ? '' : 'disabled'}>Earlier output</button>${output.history?.length ? `<button class="secondary" data-output-newer="${escapeHtml(job.id)}">Newer output</button><button class="secondary" data-output-latest="${escapeHtml(job.id)}">Latest output</button>` : ''}</div>` : ''}${(approvalCache[job.id] || []).map(approvalCard).join('')}</div>`; }).join('')}
  ${actions(last)}
  ${followTarget === last.id ? `<form data-follow-form="${escapeHtml(last.id)}" class="follow-form"><label>Follow-up message<textarea name="prompt" rows="4" required placeholder="Continue the conversation…">${escapeHtml(followDraft)}</textarea></label><div class="actions"><button type="button" data-close-follow class="secondary">Cancel</button><button class="primary">Send follow-up</button></div></form>` : ''}</div>`;
}
function questionCard(request) {
  const questions = request.details?.questions;
  if (!Array.isArray(questions) || !questions.length || questions.length > 16 || questions.some(q => typeof q.question !== 'string' || !q.question.trim()) || new Set(questions.map(q => q.question)).size !== questions.length) {
    return `<section class="approval" aria-label="Agent question"><strong>Agent question</strong><p class="error">This question cannot be displayed safely.</p><div class="actions"><button class="secondary" data-approval-id="${escapeHtml(request.id)}" data-approval-decision="decline">Deny</button></div></section>`;
  }
  if (request.decision) return `<section class="approval" aria-label="Agent question"><strong>Agent question</strong><p class="approval-result">${request.decision === 'answer' ? 'Answered' : 'Denied'}; waiting for the worker to acknowledge delivery.</p></section>`;
  return `<section class="approval" aria-label="Agent question"><strong>Agent question</strong><form data-question-form="${escapeHtml(request.id)}">${questions.map((q, index) => `<label>${escapeHtml(q.question)}${Array.isArray(q.options) && q.options.length ? `<span class="question-options">${q.options.map(o => `${escapeHtml(o.label || '')}${o.description ? ` — ${escapeHtml(o.description)}` : ''}`).join('<br>')}</span>` : ''}<textarea data-question-index="${index}" rows="2" required placeholder="${q.multiSelect ? 'Enter choices or your own answer' : 'Enter a choice or your own answer'}">${escapeHtml(questionDrafts[request.id]?.[index] || '')}</textarea></label>`).join('')}<div class="actions"><button type="button" class="secondary" data-approval-id="${escapeHtml(request.id)}" data-approval-decision="decline">Deny</button><button class="primary">Send answers</button></div></form></section>`;
}
function approvalCard(request) {
  if (request.kind === 'question') return questionCard(request);
  const label = request.kind === 'command' ? 'Command approval' : request.kind === 'question' ? 'Agent question' : request.kind === 'tool' ? 'Tool approval' : 'File change approval';
  const details = JSON.stringify(request.details, null, 2).replace(/[\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, char => `\\u${char.charCodeAt(0).toString(16).padStart(4, '0')}`);
  return `<section class="approval" aria-label="${label}"><strong>${label}</strong><p>Review the complete request before deciding. This decision applies once.</p>${request.kind === 'tool' ? '<p>Claude Code does not provide the shell’s current directory in this request. It may differ from the worktree shown above.</p>' : ''}<pre>${escapeHtml(details)}</pre>${request.decision ? `<p class="approval-result">${request.decision === 'accept' ? 'Approved' : request.decision === 'answer' ? 'Answered' : 'Denied'}; waiting for the worker to acknowledge delivery.</p>` : `${request.can_accept ? '' : '<p class="error">The request lacks complete or supported details. It can only be denied.</p>'}<div class="actions"><button class="secondary" data-approval-id="${escapeHtml(request.id)}" data-approval-decision="decline">Deny</button><button class="primary" data-approval-id="${escapeHtml(request.id)}" data-approval-decision="accept" ${request.can_accept ? '' : 'disabled'}>Allow once</button></div>`}</section>`;
}
function workRow(id, jobs) {
  const first = jobs[0], last = jobs[jobs.length - 1];
  return `<button class="work-row ${selected === id ? 'active' : ''}" data-job="${escapeHtml(id)}"><span class="job-icon">${last.runner === 'codex' ? 'C' : '✳'}</span><span class="job-main"><strong>${shorten(first.prompt)}</strong><small>${escapeHtml(last.project)} / ${escapeHtml(last.repository)} · ${escapeHtml(last.runner)}${jobs.length > 1 ? ` · ${jobs.length} turns` : ''}</small></span>${status(last.approval_pending ? 'awaiting_approval' : last.state === 'succeeded' && last.permission_denied ? 'needs_review' : last.state)}<span class="arrow">›</span></button>${selected === id ? details(jobs) : ''}`;
}
function filters() {
  return `<div class="filters"><label>Project <select id="project-filter"><option value="">All projects</option>${state.projects.map(p => `<option value="${escapeHtml(p.id)}" ${projectFilter === p.id ? 'selected' : ''}>${escapeHtml(p.name)} / ${escapeHtml(p.repository)}</option>`).join('')}</select></label><label>Node <select id="node-filter"><option value="">All nodes</option>${state.nodes.map(n => `<option value="${escapeHtml(n.id)}" ${nodeFilter === n.id ? 'selected' : ''}>${escapeHtml(n.name)}</option>`).join('')}</select></label></div>`;
}
function render() {
  $('#title').textContent = { conversations:'Conversations', queue:'Work queue', nodes:'Nodes', projects:'Projects' }[view];
  document.querySelectorAll('nav button').forEach(button => button.classList.toggle('selected', button.dataset.view === view));
  $('#new').textContent = view === 'projects' ? '+ Add project' : view === 'nodes' ? '+ Enroll worker' : '+ New work';
  let html = '';
  if (view === 'conversations' || view === 'queue') {
    const rows = view === 'queue' ? state.jobs.map(job => ({ id: job.id, jobs: [job] })) : groups();
    const visible = rows.filter(row => (!projectFilter || row.jobs.some(j => j.project_id === projectFilter)) && (!nodeFilter || row.jobs.some(j => j.node_id === nodeFilter)));
    html = `<div class="section-heading"><div><h2>${view === 'queue' ? 'All work' : 'Recent conversations'}</h2><p>${view === 'queue' ? 'Track work across your projects and nodes.' : 'Your agent work, in one place.'}</p></div><span class="count">${visible.length} total</span></div>${filters()}`;
    if (!visible.length) html += `<div class="empty"><div class="empty-icon">✳</div><h3>${state.jobs.length ? 'No matching work' : 'No work yet'}</h3><p>${state.jobs.length ? 'Try a different filter.' : 'Add a project, then start a conversation with an agent.'}</p>${state.jobs.length ? '' : '<button class="secondary" id="empty-action">Add a project</button>'}</div>`;
    else html += `<div class="work-list">${visible.map(row => workRow(row.id, row.jobs)).join('')}</div>`;
  } else if (view === 'nodes') {
    html = `<div class="section-heading"><div><h2>Worker nodes</h2><p>Machines available to run your agents.</p></div><span class="count">${state.nodes.length} total</span></div>` + (state.nodes.length ? `<div class="work-list">${state.nodes.map(n => `<div class="work-row node-row"><span class="job-icon node-icon">▣</span><span class="job-main"><strong>${escapeHtml(n.name)}</strong><small>${escapeHtml(JSON.parse(n.runners).join(', ') || 'No runners detected')} · ${state.jobs.filter(j => j.node_id === n.id && ['running','cancel_requested','unknown'].includes(j.state)).length} / ${n.capacity} occupied</small></span>${status(state.now - n.last_seen < 15 ? 'online' : 'offline')}<button data-capacity="${escapeHtml(n.id)}" class="secondary">Set capacity</button><button data-revoke="${escapeHtml(n.id)}" class="secondary">Revoke</button></div>`).join('')}</div>` : '<div class="empty"><h3>No nodes connected</h3><p>Start a worker to make it available here.</p></div>');
  } else {
    html = `<div class="section-heading"><div><h2>Projects</h2><p>Repositories available for agent work.</p></div><span class="count">${state.projects.length} total</span></div>` + (state.projects.length ? `<div class="work-list">${state.projects.map(p => `<div class="work-row"><span class="job-icon">▤</span><span class="job-main"><strong>${escapeHtml(p.name)}</strong><small>${escapeHtml(p.repository)}</small></span></div>`).join('')}</div>` : '<div class="empty"><h3>No projects yet</h3><p>Add a project and repository to get started.</p><button class="secondary" id="empty-action">Add a project</button></div>');
  }
  $('#content').innerHTML = html;
  document.querySelectorAll('[data-job]').forEach(b => b.onclick = () => { selected = selected === b.dataset.job ? null : b.dataset.job; outputCache = {}; approvalCache = {}; approvalGeneration++; render(); if (selected) { loadOutputs().catch(showError); loadApprovals().catch(showError); } });
  if ($('#project-filter')) $('#project-filter').onchange = event => { projectFilter = event.target.value; render(); };
  if ($('#node-filter')) $('#node-filter').onchange = event => { nodeFilter = event.target.value; render(); };
  if ($('#empty-action')) $('#empty-action').onclick = () => $('#project-dialog').showModal();
  document.querySelectorAll('[data-capacity]').forEach(b => b.onclick = () => { const value = window.prompt('Maximum simultaneous jobs (1–64)'); if (value !== null) act('nodes/capacity', { id:b.dataset.capacity, capacity:Number(value) }); });
  document.querySelectorAll('[data-revoke]').forEach(b => b.onclick = () => { if (window.confirm('Revoke this worker credential? The worker will lose access, including the ability to report running work.')) act('workers/revoke', { id:b.dataset.revoke }); });
  document.querySelectorAll('[data-cleanup]').forEach(b => b.onclick = () => { if (window.confirm('Remove this worktree? Git will refuse if it contains uncommitted changes. The branch will be kept.')) act('cleanup', { id:b.dataset.cleanup }); });
  document.querySelectorAll('[data-cleanup-resolve]').forEach(b => b.onclick = () => { const value = window.prompt('After checking the worker, type removed if the worktree is gone, or retained if it still exists.'); if (value === 'removed' || value === 'retained') act('cleanup/resolve', { id:b.dataset.cleanupResolve, removed:value === 'removed' }); });
  document.querySelectorAll('[data-resolve]').forEach(b => b.onclick = () => { if (window.confirm('Confirm that you have inspected the worker and stopped the agent and its child processes. Otherwise work could overlap.')) act('resolve', { id:b.dataset.resolve, process_stopped:true }); });
  document.querySelectorAll('[data-cancel]').forEach(b => b.onclick = () => act('cancel', { id:b.dataset.cancel }));
  document.querySelectorAll('[data-output-older]').forEach(b => b.onclick = () => changeOutputPage(b.dataset.outputOlder, 'older').catch(showError));
  document.querySelectorAll('[data-output-newer]').forEach(b => b.onclick = () => changeOutputPage(b.dataset.outputNewer, 'newer').catch(showError));
  document.querySelectorAll('[data-output-latest]').forEach(b => b.onclick = () => changeOutputPage(b.dataset.outputLatest, 'latest').catch(showError));
  document.querySelectorAll('[data-approval-id]').forEach(b => b.onclick = async () => {
    approvalGeneration++;
    const section = b.closest('.approval'); section.querySelectorAll('button').forEach(button => { button.disabled = true; });
    try { await api('approval/decide', {id:b.dataset.approvalId, decision:b.dataset.approvalDecision}); document.activeElement?.blur(); await refresh(); }
    catch (error) { await refresh(); showError(error); }
  });
  document.querySelectorAll('[data-question-form]').forEach(form => {
    const id = form.dataset.questionForm;
    form.querySelectorAll('[data-question-index]').forEach(input => { input.oninput = () => { (questionDrafts[id] ||= {})[input.dataset.questionIndex] = input.value; }; });
    form.onsubmit = async event => {
      event.preventDefault();
      const request = Object.values(approvalCache).flat().find(item => item.id === id && item.kind === 'question' && !item.decision);
      if (!request) { showError(Error('This question is no longer active.')); return; }
      const answers = Object.create(null);
      for (const [index, question] of request.details.questions.entries()) {
        const answer = String(questionDrafts[id]?.[index] || '').trim();
        if (!answer) { showError(Error('Answer every question.')); return; }
        answers[question.question] = answer;
      }
      form.querySelectorAll('button').forEach(button => { button.disabled = true; });
      approvalGeneration++;
      try { await api('approval/answer', {id, answers}); delete questionDrafts[id]; document.activeElement?.blur(); await refresh(); }
      catch (error) { form.querySelectorAll('button').forEach(button => { button.disabled = false; }); showError(error); }
    };
  });
  document.querySelectorAll('[data-follow]').forEach(b => b.onclick = () => { followTarget = b.dataset.follow; followDraft = ''; render(); document.querySelector('[data-follow-form] textarea')?.focus(); });
  document.querySelectorAll('[data-close-follow]').forEach(b => b.onclick = () => { followTarget = null; followDraft = ''; render(); });
  document.querySelectorAll('[data-follow-form]').forEach(form => {
    form.querySelector('textarea').oninput = event => { followDraft = event.target.value; };
    form.onsubmit = async event => { event.preventDefault(); const prompt = String(new FormData(form).get('prompt') || ''); if (!prompt.trim()) return; try { await api('followup', {id:form.dataset.followForm,prompt}); followTarget = null; followDraft = ''; await refresh(); } catch (error) { showError(error); } };
  });
}
function showError(error) { if (error.message === 'Session ended.') return; const target = document.querySelector('dialog[open] .dialog-error') || $('#notice'); target.textContent = error.message; }
async function loadOutputs() {
  const current = selected;
  const jobs = view === 'queue' ? state.jobs.filter(job => job.id === current) : groups().find(group => group.id === current)?.jobs || [];
  let changed = false;
  for (const job of jobs) {
    if (outputCache[job.id]?.history?.length) continue;
    const page = await api('output-page', {id:job.id});
    if (selected !== current) return;
    if (outputCache[job.id]?.history?.length) continue;
    const next = {...page, history:[]};
    if (JSON.stringify(outputCache[job.id]) !== JSON.stringify(next)) { outputCache[job.id] = next; changed = true; }
  }
  if (changed) renderWhenNotEditing();
}
async function loadApprovals() {
  const current = selected, currentView = view, generation = ++approvalGeneration;
  const jobs = view === 'queue' ? state.jobs.filter(job => job.id === current) : groups().find(group => group.id === current)?.jobs || [];
  const approvals = {};
  for (const job of jobs.filter(job => job.state === 'running')) approvals[job.id] = await api('approval/list', {id:job.id});
  if (selected !== current || view !== currentView || generation !== approvalGeneration) return;
  if (JSON.stringify(approvalCache) !== JSON.stringify(approvals)) { approvalCache = approvals; renderWhenNotEditing(); }
}
async function changeOutputPage(id, direction) {
  const current = selected, old = outputCache[id];
  if (!old) return;
  const history = [...old.history];
  let before;
  if (direction === 'older') { if (!old.has_more) return; history.push(old.before ?? null); before = old.next_before; }
  else if (direction === 'newer') { if (!history.length) return; before = history.pop(); }
  else { history.length = 0; before = null; }
  const page = await api('output-page', {id, before});
  if (selected !== current || outputCache[id] !== old) return;
  outputCache[id] = {...page, before:before ?? null, history};
  render();
}
function fingerprint(data) {
  return JSON.stringify({projects:data.projects,jobs:data.jobs,nodes:data.nodes.map(({last_seen,...node}) => ({...node,online:data.now-last_seen<15}))});
}
async function refresh() {
  try {
    const before = fingerprint(state); state = await api('state'); $('#notice').textContent = '';
    $('#connection').textContent = 'Connected'; $('#connection').classList.remove('disconnected');
    if (!$('#content').innerHTML || before !== fingerprint(state)) renderWhenNotEditing();
    if (selected) { await loadOutputs(); await loadApprovals(); }
  } catch (error) {
    if (token && error.message !== 'Session ended.') { $('#connection').textContent = 'Controller unavailable. Showing previously received data; actions require a connection.'; $('#connection').classList.add('disconnected'); }
    showError(error);
  }
}
async function act(path, body) { try { await api(path, body); await refresh(); } catch (error) { showError(error); } }
document.querySelectorAll('nav button').forEach(b => b.onclick = () => { view = b.dataset.view; selected = null; outputCache = {}; approvalCache = {}; approvalGeneration++; followTarget = null; followDraft = ''; render(); });
function updateNodeChoices() {
  const form = $('#jobform');
  const project = form.elements.project_id.value, runner = form.elements.runner.value;
  const select = form.elements.node_id, previous = select.value;
  const unavailable = [];
  let eligibleCount = 0;
  select.innerHTML = '<option value="">Automatic</option>' + state.nodes.map(node => {
    const missing = [];
    if (!JSON.parse(node.runners).includes(runner)) missing.push('agent unavailable');
    if (!JSON.parse(node.eligible).includes(project)) missing.push('checkout missing or invalid');
    const offline = state.now - node.last_seen >= 15;
    if (missing.length) unavailable.push(`${node.name}: ${missing.join(', ')}`); else eligibleCount++;
    const note = missing.length ? missing.join(', ') : offline ? 'offline; work will wait' : 'available';
    return `<option value="${escapeHtml(node.id)}" ${missing.length ? 'disabled' : ''}>${escapeHtml(node.name)} — ${escapeHtml(note)}</option>`;
  }).join('');
  if (previous && ![...select.options].some(option => option.value === previous)) {
    const missing = new Option('Previously selected node is unavailable', previous, false, true);
    missing.disabled = true; select.add(missing);
  }
  if ([...select.options].some(option => option.value === previous)) select.value = previous;
  const summary = state.nodes.length ? `${eligibleCount} node${eligibleCount === 1 ? '' : 's'} with a reported checkout and agent.` : 'No nodes have connected yet.';
  $('#node-eligibility').textContent = summary + (unavailable.length ? ` Unavailable: ${unavailable.join('; ')}.` : '') + (select.selectedOptions[0]?.disabled ? ' The selected node is no longer eligible; choose another node or Automatic.' : '') + ' Status refreshes as workers report.';
}
$('#jobform select[name=project_id]').addEventListener('change', updateNodeChoices);
$('#jobform select[name=runner]').addEventListener('change', updateNodeChoices);
$('#new').onclick = () => {
  if (view === 'nodes') { $('#workerform').hidden = false; $('#worker-result').hidden = true; $('#worker-dialog').showModal(); return; }
  if (view === 'projects' || !state.projects.length) { $('#project-dialog').showModal(); return; }
  $('#jobform select[name=project_id]').innerHTML = state.projects.map(p => `<option value="${escapeHtml(p.id)}">${escapeHtml(p.name)} / ${escapeHtml(p.repository)}</option>`).join('');
  updateNodeChoices();
  $('#compose').showModal();
};
document.querySelectorAll('.close').forEach(b => b.onclick = () => { const dialog = b.closest('dialog'); dialog.close(); dialog.querySelectorAll('.dialog-error').forEach(e => e.textContent = ''); if (dialog.id === 'worker-dialog') $('#worker-token').value = ''; if (dialog.id === 'compose') render(); });
$('#jobform').onsubmit = async event => { event.preventDefault(); if (event.target.elements.node_id.selectedOptions[0]?.disabled) { showError(Error('The selected node is no longer eligible. Choose another node or Automatic.')); return; } try { await api('jobs', Object.fromEntries(new FormData(event.target))); event.target.reset(); $('#compose').close(); view = 'queue'; await refresh(); } catch (error) { showError(error); } };
$('#projectform').onsubmit = async event => { event.preventDefault(); try { await api('projects', Object.fromEntries(new FormData(event.target))); event.target.reset(); $('#project-dialog').close(); await refresh(); } catch (error) { showError(error); } };
$('#workerform').onsubmit = async event => { event.preventDefault(); try { const result = await api('workers/enroll', Object.fromEntries(new FormData(event.target))); $('#workerform').hidden = true; $('#worker-result').hidden = false; $('#worker-token').value = result.token; await refresh(); } catch (error) { showError(error); } };
$('#sign-out').onclick = () => {
  if (localStorage.getItem('xenia-device-token')) fetch('/api/pairing/end', {method:'POST', headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:'{}'}).catch(() => {});
  localStorage.removeItem('xenia-device-token');
  authGeneration++; token = ''; sessionStorage.removeItem('xenia-token');
  state = { projects: [], nodes: [], jobs: [], now: 0 }; selected = null;
  outputCache = {}; approvalCache = {}; questionDrafts = {}; approvalGeneration++;
  followTarget = null; followDraft = ''; projectFilter = ''; nodeFilter = '';
  for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close();
  for (const form of document.querySelectorAll('form')) form.reset();
  $('#worker-token').value = ''; $('#notice').textContent = '';
  document.querySelectorAll('.dialog-error').forEach(error => { error.textContent = ''; });
  $('#connection').textContent = 'Signed out'; $('#connection').classList.remove('disconnected');
  view = 'conversations'; render(); $('#auth').showModal();
};
$('#authform').onsubmit = event => { event.preventDefault(); authGeneration++; localStorage.removeItem('xenia-device-token'); token = new FormData(event.target).get('token'); sessionStorage.setItem('xenia-token', token); event.target.reset(); $('#auth').close(); refresh(); };
if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js').catch(() => {});
async function redeemPairing(code) {
  $('#connection').textContent = 'Pairing this device…';
  return api('pairing/redeem', {code}).then(result => {
    authGeneration++; token = result.token; localStorage.setItem('xenia-device-token', token);
    sessionStorage.removeItem('xenia-token'); if ($('#auth').open) $('#auth').close(); refresh();
  }).catch(error => { $('#auth').showModal(); showError(error); });
}
$('#pair-linkform').onsubmit = event => {
  event.preventDefault();
  try {
    const link = new URL(new FormData(event.target).get('link'));
    const code = new URLSearchParams(link.hash.slice(1)).get('pair');
    if (link.origin !== location.origin || !/^[a-f0-9]{64}$/.test(code || '')) throw Error('Use a fresh pairing link for this controller.');
    redeemPairing(code); event.target.reset();
  } catch(error) {showError(error);}
};
window.addEventListener('hashchange', () => { const code = takePairingCode(); if (code) redeemPairing(code); });
if (pairingCode) redeemPairing(pairingCode);
else if (!token) { $('#connection').textContent = 'Signed out'; $('#auth').showModal(); } else refresh();
function eligibilityFingerprint(data) {
  return JSON.stringify(data.nodes.map(n => ({id:n.id,name:n.name,runners:n.runners,eligible:n.eligible,online:data.now-n.last_seen<15})));
}
async function refreshComposer() {
  try {
    const before = eligibilityFingerprint(state); state = await api('state');
    $('#connection').textContent = 'Connected'; $('#connection').classList.remove('disconnected');
    if (before !== eligibilityFingerprint(state)) updateNodeChoices();
  } catch (error) { if (token && error.message !== 'Session ended.') { $('#connection').textContent = 'Controller unavailable. Showing previously received data; actions require a connection.'; $('#connection').classList.add('disconnected'); } showError(error); }
}
setInterval(() => {
  if (!token || document.hidden) return;
  if ($('#compose').open) refreshComposer();
  else if (!document.querySelector('dialog[open]')) refresh();
}, 5000);

let pairingTimer;
async function loadPairedDevices() {
  const devices = await api('pairing/devices', {});
  $('#paired-devices').replaceChildren();
  if (!devices.length) { $('#paired-devices').textContent = 'No paired devices.'; return; }
  for (const device of devices) {
    const row = document.createElement('p'), button = document.createElement('button');
    row.textContent = 'Paired ' + new Date(device.created * 1000).toLocaleString() + ' ';
    button.className = 'secondary'; button.textContent = 'Revoke';
    button.onclick = async () => { try { await api('pairing/revoke', {id:device.id}); await loadPairedDevices(); } catch(error) {showError(error);} };
    row.append(button); $('#paired-devices').append(row);
  }
}
$('#pair-phone').onclick = () => {
  $('#pair-form').hidden = false; $('#pair-result').hidden = true;
  $('#pair-form input').value = location.protocol === 'https:' ? location.origin : localStorage.getItem('xenia-pair-origin') || '';
  $('#pair-dialog').showModal(); loadPairedDevices().catch(showError);
};
$('#pair-form').onsubmit = async event => {
  event.preventDefault(); const origin = String(new FormData(event.target).get('origin') || '').trim();
  const button = event.submitter; button.disabled = true;
  try {
    const result = await api('pairing/create', {origin});
    if (!$('#pair-dialog').open) { await api('pairing/cancel', {}); return; }
    $('#pair-link').value = result.url;
    localStorage.setItem('xenia-pair-origin', origin);
    $('#pair-qr').src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(result.svg);
    $('#pair-form').hidden = true; $('#pair-result').hidden = false;
    clearInterval(pairingTimer);
    const tick = () => { const seconds = Math.max(0, result.expires - Math.floor(Date.now()/1000)); $('#pair-expiry').textContent = seconds ? 'Scan within ' + seconds + ' seconds. This code works once.' : 'Code expired. Generate another code.'; if(!seconds) {$('#pair-qr').removeAttribute('src');clearInterval(pairingTimer);} };
    tick(); pairingTimer = setInterval(tick, 1000);
  } catch(error) {showError(error);} finally {button.disabled = false;}
};
$('#pair-new').onclick = () => {api('pairing/cancel',{}).catch(showError);clearInterval(pairingTimer); $('#pair-qr').removeAttribute('src');$('#pair-result').hidden=true;$('#pair-form').hidden=false;loadPairedDevices().catch(showError);};
$('#pair-dialog').addEventListener('close', () => {clearInterval(pairingTimer);$('#pair-qr').removeAttribute('src');$('#pair-link').value='';api('pairing/cancel',{}).catch(()=>{});});
