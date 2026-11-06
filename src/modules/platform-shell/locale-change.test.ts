import { describe, expect, it, vi } from 'vitest';
import { changeLocale, type LocaleChangePort } from './locale-change';

function port(currentLocale: 'en' | 'es' = 'en') {
  return {
    currentLocale: vi.fn(() => currentLocale),
    activate: vi.fn(),
    persist: vi.fn(),
    announce: vi.fn(),
  } satisfies LocaleChangePort;
}

describe('locale change transaction', () => {
  it('keeps the active locale and preference when the pack fails', async () => {
    const target = port();
    const result = await changeLocale(
      'es',
      1,
      () => 1,
      target,
      async () => {
        throw new Error('offline');
      },
    );

    expect(result).toBe('kept');
    expect(target.currentLocale).toHaveReturnedWith('en');
    expect(target.activate).not.toHaveBeenCalled();
    expect(target.persist).not.toHaveBeenCalled();
    expect(target.announce).toHaveBeenCalledWith(
      'The language pack could not be loaded. Keeping the previous language.',
    );
  });

  it('commits the new locale only after a complete pack loads', async () => {
    const target = port();
    const result = await changeLocale('es', 1, () => 1, target);

    expect(result).toBe('changed');
    expect(target.activate).toHaveBeenCalledWith('es');
    expect(target.persist).toHaveBeenCalledWith('es');
    expect(target.announce).toHaveBeenCalledWith('Idioma cambiado a Español');
  });

  it('drops a late successful result after a newer request starts', async () => {
    const target = port();
    let latest = 1;
    const pending = changeLocale('es', 1, () => latest, target);
    latest = 2;

    expect(await pending).toBe('superseded');
    expect(target.activate).not.toHaveBeenCalled();
    expect(target.persist).not.toHaveBeenCalled();
  });

  it('does not announce a stale failure over a newer language request', async () => {
    const target = port();
    let latest = 1;
    const pending = changeLocale('es', 1, () => latest, target, async () => {
      throw new Error('late failure');
    });
    latest = 2;

    expect(await pending).toBe('superseded');
    expect(target.announce).not.toHaveBeenCalled();
    expect(target.activate).not.toHaveBeenCalled();
    expect(target.persist).not.toHaveBeenCalled();
  });
});
