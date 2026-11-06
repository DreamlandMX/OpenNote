import type { SupportedLocaleId } from '../presentation-i18n';
import { SUPPORTED_LOCALES } from '../presentation-i18n';

export interface LocaleSelectProps {
  id: string;
  label: string;
  value: SupportedLocaleId;
  describedById?: string;
  onChange: (locale: SupportedLocaleId) => void | Promise<void>;
}

export function LocaleSelect({ id, label, value, describedById, onChange }: LocaleSelectProps) {
  return (
    <div className="form-row">
      <label htmlFor={id}>{label}</label>
      <select
        id={id}
        value={value}
        aria-describedby={describedById}
        onChange={(event) => {
          const next = event.target.value as SupportedLocaleId;
          if (SUPPORTED_LOCALES.some((l) => l.id === next)) {
            void Promise.resolve(onChange(next))
              .catch(() => undefined);
          }
        }}
      >
        {SUPPORTED_LOCALES.map((locale) => (
          <option key={locale.id} value={locale.id}>
            {locale.selfName}
          </option>
        ))}
      </select>
    </div>
  );
}
