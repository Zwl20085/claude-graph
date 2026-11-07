/** Settings → Harnesses: the machine's profiles (kind badge, built-in marker) and an edit form. */
import { useState } from 'react';
import { Plus } from 'lucide-react';
import type { HarnessProfile } from '../types/contract';
import { findHarness, HARNESS_KIND_LABELS, newCustomHarness, nextCustomId } from '../lib/harness';
import { useGraphStore } from '../store/graphStore';
import { HarnessForm } from './HarnessForm';
import { btnCls, Modal } from './ui';

function ProfileRow({ profile, selected, onSelect }: { profile: HarnessProfile; selected: boolean; onSelect: () => void }) {
  return (
    <li>
      <button
        className={`flex w-full items-center gap-1.5 rounded px-2 py-1 text-left text-[12px] ${selected ? 'bg-[#37373d] text-white' : 'text-[#d4d4d4] hover:bg-[#2d2d30]'}`}
        aria-current={selected ? 'true' : undefined}
        aria-label={`edit harness ${profile.label}`}
        onClick={onSelect}
      >
        <span className="min-w-0 flex-1 truncate">{profile.label}</span>
        <span className="shrink-0 rounded bg-[#1e1e1e] px-1 text-[10px] text-[#9a9a9a]" aria-label={`kind ${profile.kind}`}>
          {HARNESS_KIND_LABELS[profile.kind]}
        </span>
        {profile.builtin && (
          <span className="shrink-0 rounded border border-[#3c3c3c] px-1 text-[10px] text-[#8a8a8a]" aria-label="built in">
            built-in
          </span>
        )}
      </button>
    </li>
  );
}

export function HarnessDialog({ onClose }: { onClose: () => void }) {
  const profiles = useGraphStore((s) => s.harnesses);
  const [selectedId, setSelectedId] = useState<string | null>(profiles[0]?.id ?? null);
  const [draftNew, setDraftNew] = useState<HarnessProfile | null>(null);
  const selected = draftNew ?? (selectedId === null ? null : findHarness(profiles, selectedId));

  const startNew = (): void => {
    setDraftNew(newCustomHarness(nextCustomId(profiles)));
    setSelectedId(null);
  };
  const select = (id: string): void => {
    setDraftNew(null);
    setSelectedId(id);
  };

  return (
    <Modal title="Harnesses" onClose={onClose}>
      <div className="flex min-h-[360px] gap-3">
        <nav className="w-[220px] shrink-0 space-y-2" aria-label="Harness profiles">
          <ul className="space-y-0.5">
            {profiles.map((p) => (
              <ProfileRow key={p.id} profile={p} selected={draftNew === null && p.id === selectedId} onSelect={() => select(p.id)} />
            ))}
            {draftNew && <ProfileRow profile={draftNew} selected onSelect={() => undefined} />}
          </ul>
          <button className={`${btnCls} border border-[#3c3c3c]`} onClick={startNew} disabled={draftNew !== null}>
            <Plus size={13} />New custom harness
          </button>
          <p className="text-[11px] text-[#8a8a8a]">Profiles live on this machine, not in graph.json.</p>
        </nav>
        <div className="min-w-0 flex-1">
          {selected ? (
            <HarnessForm
              key={draftNew ? `new-${draftNew.id}` : `${selected.id}:${JSON.stringify(selected)}`}
              profile={selected}
              isNew={draftNew !== null}
              takenIds={profiles.map((p) => p.id)}
              onSaved={select}
              onDeleted={() => select(profiles.find((p) => p.builtin)?.id ?? '')}
            />
          ) : (
            <p className="text-[12px] text-[#777]">Select a profile.</p>
          )}
        </div>
      </div>
    </Modal>
  );
}
