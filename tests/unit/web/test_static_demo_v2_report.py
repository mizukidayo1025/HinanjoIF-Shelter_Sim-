"""The report page's script (demo-v2-report.js): the report's summary panel."""

import shutil
import subprocess
from pathlib import Path

import pytest

STATIC = Path("src/sheltersim/web/static")


def test_the_report_summary_is_of_the_whole_range_and_shows_no_ai_box_without_ai():
    """#610: the panel's 「記録から確認できること」 tells the branch's whole computed range (the
    summary of #612), not the minute shown; a report the rules made (every report of the browser)
    is said to be made from the record, and no box tells of an AI the report did not ask."""
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is required for the UI callback regression")
    script = r"""
import fs from 'node:fs';
import assert from 'node:assert/strict';
const read = (path) => fs.readFileSync(path, 'utf8').replaceAll('\r\n', '\n');
const source = read(process.argv[1]), text = read(process.argv[2]);
const body = source.split('  async function loadReport() {')[1]
  .split('  // The questions asked about this branch before')[0];
const lasting = eval('(' + text.split('  const lasting = ')[1].split('\n  };\n')[0] + '\n  })');
const node = (tag, text, cls) => ({
  tag, text, cls, children: [], textContent: '',
  append(...nodes) { this.children.push(...nodes); },
  replaceChildren(...nodes) { this.children = nodes; },
  insertBefore(child) { this.children.unshift(child); },
});
const nodes = {};
const $ = (id) => (nodes[id] ??= node('div'));
const el = node, uiIcon = () => node('icon'), json = JSON.stringify;
const time = (m) => `${m}分`, num = v => Number.isFinite(v) ? String(Math.round(v * 10) / 10) : '—';
// The minute shown is 0, where nothing has happened: the panel must not tell of it.
const state = { scope: 1, branch: 'main', minute: 0,
  frame: { kpis: { stress_mean: 0, toilet_queue_length: 0, unresolved_support_request_count: 0 },
           people: [], staff: [], events: [] } };
const reference = () => ({ fork_minute: 0, computed_until: 10080 });
const asked = [];
const answers = {
  report: {
    status: 'rule_fallback', markdown: '# report',
    review: { review: { recommendations: [] } },
  },
  summary: { sample_count: 10080, oldest_minute: 0, newest_minute: 10079, metrics: {
    stress_mean: { mean: 0.33, max: 0.519 },
    toilet_queue_length: { max: 21, positive_minutes: 3180 },
    unresolved_support_request_count: { max: 33, positive_minutes: 10079 },
  } },
};
const api = async (path) => {
  asked.push(path);
  if (path.includes('/history?')) return answers.summary;
  if (path.endsWith('/report')) return answers.report;
  return {};
};
const branchBase = () => '/b', runBase = () => '/r';
const syncReportBranches = () => {}, renderReportSummary = () => {}, renderEvents = () => {};
const loadConversations = async () => {}, loadHistory = async () => {}, drawAll = () => {};
const personSummary = () => '';
const loadReport = eval('(async function loadReport() {' + body + ')');
await loadReport();
assert.ok(asked.includes('/b/history?kind=summary&start=0&end=10080'), asked.join(' '));
assert.equal($('report-status').textContent, '記録から作成（AIなし）');
const boxes = $('report-summary').children.filter((child) => child.tag === 'div');
assert.deepEqual(boxes.map((box) => box.children[0].text),
  ['記録から確認できること', '課題と次の検討', '解釈上の限界']);
assert.equal(boxes[0].children[1].text,
  '0分〜10079分の全体で、避難者の負担は平均 33/100（最大 51.9）。' +
  'トイレの列は最大 21人で、列があった時間は計 2日5時間。' +
  '対応待ちの相談は最大 33件で、対応待ちがあった時間は計 6日23時間59分。');
assert.match(boxes[1].children[1].text, /^対応待ちの相談が続いた時間帯の/);
// A branch with no samples says so.
answers.summary = { sample_count: 0, metrics: {} };
await loadReport();
const again = $('report-summary').children.filter((child) => child.tag === 'div');
assert.equal(again[0].children[1].text, 'この分岐には指標の記録がありません。');
assert.equal(again[1].children[1].text,
  '記録された事象を確認し、別の条件での結果と比較してください。');
"""
    subprocess.run(
        [
            node,
            "--input-type=module",
            "-e",
            script,
            str((STATIC / "demo-v2-report.js").resolve()),
            str((STATIC / "demo-v2-text.js").resolve()),
        ],
        check=True,
    )
