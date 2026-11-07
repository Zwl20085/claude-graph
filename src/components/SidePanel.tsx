/** Right aside: tab strip `Assistant | Node` over the AssistantPanel or the node Inspector. */
import { AssistantPanel } from './AssistantPanel';
import { Inspector } from './Inspector';

interface SidePanelProps {
  assistant: boolean;
  onSelectTab: (assistant: boolean) => void;
}

const TABS = [
  { assistant: true, label: 'Assistant', id: 'side-tab-assistant' },
  { assistant: false, label: 'Node', id: 'side-tab-node' },
] as const;

export function SidePanel({ assistant, onSelectTab }: SidePanelProps) {
  const current = TABS.find((t) => t.assistant === assistant) ?? TABS[0];
  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 border-b border-[#3c3c3c] text-[12px]" role="tablist" aria-label="Side panel">
        {TABS.map((t) => (
          <button
            key={t.id}
            id={t.id}
            role="tab"
            aria-selected={t.assistant === assistant}
            aria-controls="side-tabpanel"
            className={`px-3 py-1.5 ${t.assistant === assistant ? 'border-b-2 border-[#3b82f6] text-white' : 'text-[#9a9a9a] hover:text-[#d4d4d4]'}`}
            onClick={() => onSelectTab(t.assistant)}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1" role="tabpanel" id="side-tabpanel" aria-labelledby={current.id}>
        {assistant ? <AssistantPanel /> : <Inspector />}
      </div>
    </div>
  );
}
