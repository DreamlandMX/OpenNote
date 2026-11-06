import { BookOpen, House, Settings as SettingsIcon } from 'lucide-react';
import type { Translator } from '../presentation-i18n';

export type Screen = 'welcome' | 'home' | 'workspace' | 'settings';

export interface SidebarProps {
  translator: Translator;
  screen: Exclude<Screen, 'welcome'>;
  drawerOpen: boolean;
  showWorkspace: boolean;
  onNavigate: (screen: Exclude<Screen, 'welcome'>) => void;
  statusTop: string;
  statusBottom: string;
}

export function Sidebar({
  translator,
  screen,
  drawerOpen,
  showWorkspace,
  onNavigate,
  statusTop,
  statusBottom,
}: SidebarProps) {
  const items = [
    { id: 'home' as const, label: translator('shell.nav.home'), icon: House },
    ...(showWorkspace
      ? [{ id: 'workspace' as const, label: translator('shell.nav.workspace'), icon: BookOpen }]
      : []),
    { id: 'settings' as const, label: translator('shell.nav.settings'), icon: SettingsIcon },
  ];
  return (
    <nav className="drawer" aria-label={translator('shell.brand.name')} data-open={drawerOpen ? 'true' : 'false'}>
      <ul className="nav-list">
        {items.map((item) => (
          <li key={item.id}>
            <button
              type="button"
              className="nav-item"
              aria-label={item.label}
              title={item.label}
              aria-current={screen === item.id ? 'true' : undefined}
              onClick={() => onNavigate(item.id)}
            >
              <item.icon size={18} aria-hidden="true" />
              <span>{item.label}</span>
            </button>
          </li>
        ))}
      </ul>
      <p className="sidebar-status">
        <strong>{statusTop}</strong>
        <br />
        <span>{statusBottom}</span>
      </p>
    </nav>
  );
}
