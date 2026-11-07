import { describe, expect, it } from 'vitest';
import example from '../../examples/hello-pipeline/graph.json';
import type { Graph } from '../types/contract';
import { displayDir, renderBrief } from './brief';
import { addNode, emptyGraph, updateNode } from './graphOps';
import { validateGraph } from './validate';

const load = (): Graph => {
  const g = validateGraph(JSON.parse(JSON.stringify(example)));
  if (g instanceof Error) throw g;
  return g;
};

const HEADINGS = [
  '## Task',
  '## Goal',
  '## Acceptance criterion',
  '## Inputs from upstream nodes',
  '## Scope',
  '## Already known / ruled out',
  '## Deliverables (required)',
  '## Do not',
  '## Extra context',
  '## How to finish',
];

describe('renderBrief (preview)', () => {
  it('keeps the template section order with every field filled', () => {
    const g = updateNode(load(), 'design', { prompt: 'Do the design.\n  keep indentation' });
    const text = renderBrief(g, 'design', { graphDir: '/w/g' });
    const positions = HEADINGS.map((h) => text.indexOf(`\n${h}\n`));
    expect(positions.every((p) => p > 0)).toBe(true);
    expect([...positions].sort((a, b) => a - b)).toEqual(positions);
    expect(text.startsWith('# Node: Layout rule set   (graph: hello-pipeline, node id: design)\n')).toBe(true);
    expect(text).toContain('## Task\nDo the design.\n  keep indentation\n');
    expect(text).toContain('Your deliverables folder: /w/g/nodes/design/deliverables/   (write ALL outputs here)');
    expect(text).toContain('Working directory: /w/g\n');
    expect(text).toContain('\n## Deliverables\n- <path> — one line on what it contains\n## Status: DONE\n');
  });

  it('prompt-only node: Task, upstream, short finish; no empty structured sections', () => {
    const { graph, id } = addNode(emptyGraph('g'), 'Solo', { x: 0, y: 0 });
    const text = renderBrief(updateNode(graph, id, { prompt: 'Fix the bug' }), id, { graphDir: 'C:\\w\\g\\' });
    expect(text).toContain('## Task\nFix the bug\n');
    expect(text).toContain('None. This is a root node.');
    for (const h of HEADINGS.slice(1).filter((h) => h !== '## Inputs from upstream nodes' && h !== '## How to finish')) {
      expect(text).not.toContain(h);
    }
    expect(text).not.toContain('## Deliverables');
    expect(text).toContain('When the task is complete, end your final message with exactly:\n\n## Status: DONE');
    expect(text).toContain('Graph folder: C:/w/g\n');
  });

  it('lists upstream nodes with absolute deliverable paths and truncated results', () => {
    const text = renderBrief(load(), 'design', {
      graphDir: '/w/g',
      upstreamStatus: { survey: 'succeeded' },
      upstreamResults: { survey: 'x'.repeat(5000) },
    });
    expect(text).toContain('### Literature survey (id: survey) — status: succeeded');
    expect(text).toContain('- /w/g/nodes/survey/deliverables/survey.md — Markdown table of 10 papers with DOI and two-line summary');
    expect(text).toContain(`\n${'x'.repeat(4000)}\n`);
    expect(text).not.toContain('x'.repeat(4001));
    // Same fence as prompt.rs (src-tauri/tests/fixtures/brief_design.md).
    const fenced = renderBrief(load(), 'design', {
      graphDir: '/w/g',
      upstreamStatus: { survey: 'succeeded' },
      upstreamResults: { survey: 'Survey done.\n\n## Status: DONE\n' },
    });
    expect(fenced).toContain('Result summary (first 4000 chars of its result.md):\n```text\nSurvey done.\n\n## Status: DONE\n```\n');
  });

  it('uses placeholders when run-time facts are absent', () => {
    const text = renderBrief(load(), 'report');
    expect(text).toContain('Graph folder: <graph folder>');
    expect(text).toContain('status: not run yet');
    expect(text).toContain('(no result.md available)');
    expect(renderBrief(load(), 'survey')).toContain('## Do not\n- Do not install packages\n');
  });

  it('working_dir absolute or relative; unknown node throws', () => {
    const g = load();
    expect(renderBrief(updateNode(g, 'survey', { working_dir: 'D:\\code' }), 'survey')).toContain('Working directory: D:/code');
    expect(renderBrief(updateNode(g, 'survey', { working_dir: 'sub' }), 'survey', { graphDir: '/g' })).toContain(
      'Working directory: /g/sub',
    );
    expect(() => renderBrief(g, 'ghost')).toThrow(/Unknown node/);
    expect(displayDir('/')).toBe('/');
  });
});
