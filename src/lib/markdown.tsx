/**
 * A small, safe Markdown renderer for result.md / reply.md: headings, lists,
 * quotes, fenced code, paragraphs, and inline `code`, **bold**, *italic*.
 * Everything becomes React text nodes: no raw HTML, no links are followed.
 */
import type { ReactNode } from 'react';

type Block =
  | { kind: 'heading'; level: number; text: string }
  | { kind: 'code'; text: string }
  | { kind: 'list'; ordered: boolean; items: string[] }
  | { kind: 'quote'; text: string }
  | { kind: 'para'; text: string };

const FENCE = /^(`{3,}|~{3,})/;
const HEADING = /^(#{1,6})\s+(.*)$/;
const BULLET = /^\s*[-*+]\s+(.*)$/;
const NUMBERED = /^\s*\d+[.)]\s+(.*)$/;

export function parseBlocks(md: string): Block[] {
  const lines = md.replace(/\r\n/g, '\n').split('\n');
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] ?? '';
    const fence = FENCE.exec(line);
    if (fence) {
      const close = fence[1] ?? '```';
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !(lines[i] ?? '').startsWith(close)) body.push(lines[i++] ?? '');
      i += 1;
      out.push({ kind: 'code', text: body.join('\n') });
      continue;
    }
    const h = HEADING.exec(line);
    if (h) {
      out.push({ kind: 'heading', level: (h[1] ?? '#').length, text: h[2] ?? '' });
      i += 1;
      continue;
    }
    if (BULLET.test(line) || NUMBERED.test(line)) {
      const ordered = NUMBERED.test(line);
      const re = ordered ? NUMBERED : BULLET;
      const items: string[] = [];
      while (i < lines.length && re.test(lines[i] ?? '')) items.push(re.exec(lines[i++] ?? '')?.[1] ?? '');
      out.push({ kind: 'list', ordered, items });
      continue;
    }
    if (line.startsWith('>')) {
      const body: string[] = [];
      while (i < lines.length && (lines[i] ?? '').startsWith('>')) body.push((lines[i++] ?? '').replace(/^>\s?/, ''));
      out.push({ kind: 'quote', text: body.join('\n') });
      continue;
    }
    if (line.trim() === '') {
      i += 1;
      continue;
    }
    const body: string[] = [];
    while (i < lines.length) {
      const l = lines[i] ?? '';
      if (l.trim() === '' || FENCE.test(l) || HEADING.test(l) || BULLET.test(l) || NUMBERED.test(l) || l.startsWith('>')) break;
      body.push(l);
      i += 1;
    }
    out.push({ kind: 'para', text: body.join('\n') });
  }
  return out;
}

const INLINE = /(`[^`]+`|\*\*[^*]+\*\*|\*[^*\s][^*]*\*)/g;

export function inline(text: string): ReactNode[] {
  return text.split(INLINE).map((part, i) => {
    if (part.startsWith('`') && part.endsWith('`') && part.length > 1) {
      return <code key={i} className="rounded bg-[#1e1e1e] px-1 font-mono">{part.slice(1, -1)}</code>;
    }
    if (part.startsWith('**') && part.endsWith('**') && part.length > 3) return <strong key={i}>{part.slice(2, -2)}</strong>;
    if (part.startsWith('*') && part.endsWith('*') && part.length > 2) return <em key={i}>{part.slice(1, -1)}</em>;
    return part;
  });
}

const HEADING_CLS = ['text-[14px]', 'text-[13px]', 'text-[12px]', 'text-[12px]', 'text-[12px]', 'text-[12px]'];

export function Markdown({ text }: { text: string }) {
  return (
    <div className="space-y-1.5 text-[12px] text-[#d4d4d4]" data-testid="markdown">
      {parseBlocks(text).map((b, i) => {
        switch (b.kind) {
          case 'heading':
            return (
              <p key={i} role="heading" aria-level={b.level} className={`${HEADING_CLS[b.level - 1] ?? 'text-[12px]'} font-semibold text-white`}>
                {inline(b.text)}
              </p>
            );
          case 'code':
            return <pre key={i} className="overflow-auto whitespace-pre-wrap rounded bg-[#1e1e1e] p-2 font-mono text-[11px]">{b.text}</pre>;
          case 'list': {
            const items = b.items.map((it, j) => <li key={j}>{inline(it)}</li>);
            return b.ordered ? <ol key={i} className="list-decimal pl-5">{items}</ol> : <ul key={i} className="list-disc pl-5">{items}</ul>;
          }
          case 'quote':
            return <blockquote key={i} className="whitespace-pre-wrap border-l-2 border-[#3c3c3c] pl-2 text-[#9a9a9a]">{inline(b.text)}</blockquote>;
          case 'para':
            return <p key={i} className="whitespace-pre-wrap">{inline(b.text)}</p>;
          default:
            return null;
        }
      })}
    </div>
  );
}
