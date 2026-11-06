import { useState } from 'react';
import type { Translator, SupportedLocaleId } from '../presentation-i18n';
import type { ThemePreference } from './device-prefs';
import {
  bodySizeLabel,
  clampBodySize,
  clampLineMeasure,
  DEFAULT_BODY_SIZE,
  defaultMeasureFor,
  MAX_BODY_SIZE,
  MAX_LINE_MEASURE,
  MIN_BODY_SIZE,
  MIN_LINE_MEASURE,
} from './appearance';
import { LocaleSelect } from './LocaleSelect';
import { WEEKDAY_KEYS } from './daily-view';
import { ShortcutSettings } from './ShortcutSettings';
import type { BindableCommandId, Chord } from './shortcuts';
import {
  COMMON_TIME_ZONES,
  firstDayOfWeek,
  formatCalendarDate,
  formatNumber,
  zoneLabel,
  type RegionalPreferences,
} from './regional';

export interface SettingsScreenProps {
  onBack?: () => void;
  translator: Translator;
  locale: SupportedLocaleId;
  onLocaleChange: (locale: SupportedLocaleId) => void | Promise<void>;
  currentTheme: ThemePreference;
  onThemeChange: (theme: ThemePreference) => void;
  /** F29: null means "use the default for this script". */
  bodySize: number | null;
  lineMeasure: number | null;
  onBodySizeChange: (size: number | null) => void;
  onLineMeasureChange: (measure: number | null) => void;
  /** F30: null follows the system reduced-motion setting. */
  reduceMotion: boolean | null;
  onReduceMotionChange: (value: boolean | null) => void;
  /** F30 stored chords; the settings panel edits this map. */
  shortcutBindings: Readonly<Partial<Record<BindableCommandId, Chord>>>;
  onShortcutBindingsChange(bindings: Partial<Record<BindableCommandId, Chord>>): void;
  onOpenShortcuts(): void;
  /** F31 resolved preferences, already merged with the interface locale. */
  regional: RegionalPreferences;
  onRegionChange(region: string | null): void;
  onTimeZoneChange(timeZone: string | null): void;
}

const THEME_OPTIONS: readonly ThemePreference[] = ['system', 'light', 'dark'];

export function SettingsScreen({
  onBack,
  translator,
  locale,
  onLocaleChange,
  currentTheme,
  onThemeChange,
  bodySize,
  lineMeasure,
  onBodySizeChange,
  onLineMeasureChange,
  reduceMotion,
  onReduceMotionChange,
  shortcutBindings,
  onShortcutBindingsChange,
  onOpenShortcuts,
  regional,
  onRegionChange,
  onTimeZoneChange,
}: SettingsScreenProps) {
  const [category, setCategory] = useState('general');
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const effectiveBody = bodySize ?? DEFAULT_BODY_SIZE;
  const effectiveMeasure = lineMeasure ?? defaultMeasureFor(locale);
  return (
    <div className="settings content-panel narrow">
            <nav className="settings-categories" aria-label={translator('settings.title')}>
        {onBack ? <button type="button" className="secondary" onClick={onBack}>{translator('home.back')}</button> : null}
        {(['general', 'appearance', 'editor', 'shortcuts', 'advanced'] as const).map((item) => <button key={item} type="button" aria-pressed={category === item} onClick={() => setCategory(item)}>{translator(`settings.category.${item}`)}</button>)}
      </nav>
      <div className="settings-detail">
      <h1>{translator('settings.title')}</h1>
      <section hidden={category !== 'general'}>
        <LocaleSelect
          id="settings-locale"
          label={translator('settings.language.label')}
          describedById="settings-language-help"
          value={locale}
          onChange={onLocaleChange}
        />
        <p id="settings-language-help" className="notice">
          {translator('settings.language.hint')}
        </p>
      </section>
      <section hidden={category !== 'appearance'}>
        <form>
          <fieldset>
            <legend>{translator('settings.theme.label')}</legend>
            {THEME_OPTIONS.map((option) => (
              <label key={option} className="theme-option">
                <input
                  type="radio"
                  name="theme"
                  value={option}
                  checked={currentTheme === option}
                  onChange={() => onThemeChange(option)}
                />
                {translator(`settings.theme.${option}`)}
              </label>
            ))}
          </fieldset>
        </form>
      </section>
      <section hidden={category !== 'editor'}>
        <h2>{translator('settings.reading.label')}</h2>
        <label className="appearance-field" htmlFor="settings-body-size">
          <span>{translator('settings.reading.bodySize')}</span>
          <input
            id="settings-body-size"
            type="range"
            min={MIN_BODY_SIZE}
            max={MAX_BODY_SIZE}
            step={1}
            value={effectiveBody}
            aria-valuetext={bodySizeLabel(effectiveBody)}
            onChange={(event) => onBodySizeChange(clampBodySize(Number(event.target.value)))}
          />
          <output htmlFor="settings-body-size">{bodySizeLabel(effectiveBody)}</output>
        </label>
        <label className="appearance-field" htmlFor="settings-line-measure">
          <span>{translator('settings.reading.lineMeasure')}</span>
          <input
            id="settings-line-measure"
            type="range"
            min={MIN_LINE_MEASURE}
            max={MAX_LINE_MEASURE}
            step={1}
            value={effectiveMeasure}
            aria-valuetext={translator('settings.reading.lineMeasureValue', { count: clampLineMeasure(effectiveMeasure) })}
            onChange={(event) => onLineMeasureChange(clampLineMeasure(Number(event.target.value)))}
          />
          <output htmlFor="settings-line-measure">{translator('settings.reading.lineMeasureValue', { count: clampLineMeasure(effectiveMeasure) })}</output>
        </label>
        <button
          type="button"
          className="secondary"
          onClick={() => {
            // Reset returns to the script default rather than a fixed number, so
            // switching to Japanese later still gets the CJK measure.
            onBodySizeChange(null);
            onLineMeasureChange(null);
          }}
        >
          {translator('settings.reading.reset')}
        </button>
        <p className="notice">{translator('settings.reading.hint')}</p>
      </section>
      <section hidden={category !== 'appearance'}>
        <h2>{translator('settings.motion.label')}</h2>
        <fieldset className="motion-options">
          <legend className="visually-hidden">{translator('settings.motion.label')}</legend>
          {[
            { value: null, key: 'settings.motion.system' as const },
            { value: true, key: 'settings.motion.reduce' as const },
            { value: false, key: 'settings.motion.full' as const },
          ].map((option) => (
            <label key={option.key} className="theme-option">
              <input
                type="radio"
                name="reduce-motion"
                checked={reduceMotion === option.value}
                onChange={() => onReduceMotionChange(option.value)}
              />
              {translator(option.key)}
            </label>
          ))}
        </fieldset>
        <p className="notice">{translator('settings.motion.hint')}</p>
      </section>
      <section hidden={category !== 'advanced'}>
        <h2>{translator('settings.regional.label')}</h2>
        <label className="appearance-field" htmlFor="settings-region">
          <span>{translator('settings.regional.region')}</span>
          <input
            id="settings-region"
            type="text"
            value={regional.region}
            onChange={(event) => onRegionChange(event.target.value.trim() || null)}
          />
        </label>
        <p className="notice">{translator('settings.regional.regionHint')}</p>
        <label className="appearance-field" htmlFor="settings-zone">
          <span>{translator('settings.regional.timeZone')}</span>
          <select
            id="settings-zone"
            value={regional.timeZone}
            onChange={(event) => onTimeZoneChange(event.target.value)}
          >
            {COMMON_TIME_ZONES.map((zone) => (
              <option key={zone} value={zone}>
                {zone}
              </option>
            ))}
          </select>
        </label>
        <p className="notice">{translator('settings.regional.zoneHint')}</p>
        <div className="regional-preview" role="group" aria-label={translator('settings.regional.preview')}>
          <p>{translator('settings.regional.preview')}</p>
          <dl>
            <dt>{translator('settings.regional.date')}</dt>
            <dd>
              <time dateTime="2026-10-20">{formatCalendarDate('2026-10-20', regional.region)}</time>
            </dd>
            <dt>{translator('settings.regional.number')}</dt>
            <dd>{formatNumber(1234567, regional.region)}</dd>
            <dt>{translator('settings.regional.weekStart')}</dt>
            <dd>{translator(WEEKDAY_KEYS[firstDayOfWeek(regional.region) - 1] ?? 'daily.weekday.0')}</dd>
            <dt>{translator('settings.regional.zone')}</dt>
            <dd>{zoneLabel(regional)}</dd>
          </dl>
          <p className="notice">{translator('settings.regional.previewHint')}</p>
        </div>
      </section>
      <section hidden={category !== 'shortcuts'}>
        <h2>{translator('shortcut.title')}</h2>
        <button
          type="button"
          className="secondary"
          aria-haspopup="dialog"
          aria-expanded={shortcutsOpen ? 'true' : 'false'}
          onClick={() => {
            setShortcutsOpen(true);
            onOpenShortcuts();
          }}
        >
          {translator('shortcut.edit')}
        </button>
        {shortcutsOpen ? (
          <ShortcutSettings
            bindings={shortcutBindings}
            translator={translator}
            onChange={onShortcutBindingsChange}
            onClose={() => setShortcutsOpen(false)}
          />
        ) : null}
      </section>
      </div>
    </div>
  );
}
