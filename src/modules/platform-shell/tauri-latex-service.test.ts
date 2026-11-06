import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { InvokeArgs } from '@tauri-apps/api/core';
import {
  ARTIFACT_CHUNK_BYTES,
  createTauriLatexBuilds,
  createTauriLatexEnvironment,
  createTauriLatexService,
  disconnectedLatexService,
} from './tauri-latex-service';

const invokeMock = vi.hoisted(() => vi.fn<(command: string, args?: InvokeArgs) => Promise<unknown>>());
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

describe('tauri latex project service', () => {
  // A block body: a function returned from beforeEach would run as cleanup.
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it('addresses projects by selection, project and file ids, never by path', async () => {
    const service = createTauriLatexService();
    invokeMock.mockResolvedValue({ ok: 'value' });
    await service.chooseProjectFolder('v1', 'Pick');
    await service.inspectProject('v1', 's1');
    await service.registerProject('v1', 's1', 'main.tex', 'op1');
    await service.openProject('v1', 'p1');
    await service.readProjectFile('v1', 'p1', 'f1');
    await service.saveProjectFile('v1', 'p1', 'f1', '\\section{A}', 3, 'op2');
    expect(invokeMock.mock.calls).toEqual([
      ['select_latex_project_folder', { vaultId: 'v1', dialogTitle: 'Pick' }],
      ['inspect_latex_folder', { vaultId: 'v1', selectionId: 's1' }],
      ['register_latex_project', { vaultId: 'v1', selectionId: 's1', entryPath: 'main.tex', operationId: 'op1' }],
      ['open_latex_project', { vaultId: 'v1', projectId: 'p1' }],
      ['read_latex_project_file', { vaultId: 'v1', projectId: 'p1', fileId: 'f1' }],
      [
        'save_latex_project_file',
        { vaultId: 'v1', projectId: 'p1', fileId: 'f1', text: '\\section{A}', expectedRevision: 3, operationId: 'op2' },
      ],
    ]);
  });

  it('lists, removes and creates projects through their own commands', async () => {
    const service = createTauriLatexService();
    invokeMock.mockResolvedValue(null);
    const metadata = { title: 'T', author: '', date: '2026-10-30', language: 'en' as const, paper: 'a4' as const };
    await service.listProjects('v1');
    await service.unregisterProject('v1', 'p1', 'op1');
    await service.createProject('v1', { templateId: 'report', metadata, destination: 'papers/t' }, 'op2');
    expect(invokeMock.mock.calls).toEqual([
      ['list_latex_projects', { vaultId: 'v1' }],
      ['unregister_latex_project', { vaultId: 'v1', projectId: 'p1', operationId: 'op1' }],
      ['create_latex_project', { vaultId: 'v1', templateId: 'report', metadata, destination: 'papers/t', operationId: 'op2' }],
    ]);
  });

  it('prepares only through an explicit consent argument', async () => {
    const environment = createTauriLatexEnvironment();
    invokeMock.mockResolvedValue(null);
    await environment.plan();
    await environment.status();
    await environment.prepare(true, 'op');
    await environment.cancel();
    await environment.remove();
    expect(invokeMock.mock.calls).toEqual([
      ['latex_environment_plan', {}],
      ['latex_environment_status', {}],
      ['prepare_latex_environment', { consent: true, operationId: 'op' }],
      ['cancel_latex_preparation', {}],
      ['remove_latex_environment', {}],
    ]);
  });

  it('builds, cancels and reads status only through host commands, naming the shown manifest', async () => {
    const builds = createTauriLatexBuilds();
    invokeMock.mockResolvedValue(null);
    await builds.compile('v1', 'p1', 'm1', 'op1');
    await builds.cancel('v1', 'job-1');
    await builds.status('v1', 'p1');
    expect(invokeMock.mock.calls).toEqual([
      ['compile_latex_project', { vaultId: 'v1', projectId: 'p1', expectedManifestHash: 'm1', operationId: 'op1' }],
      ['cancel_latex_compile', { vaultId: 'v1', jobId: 'job-1' }],
      ['latex_build_status', { vaultId: 'v1', projectId: 'p1' }],
    ]);
  });

  it('reads a PDF in bounded chunks and accepts it only if it hashes to the published bytes', async () => {
    const total = ARTIFACT_CHUNK_BYTES + 1000;
    const pdf = new Uint8Array(total).map((_, index) => index % 251);
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', pdf));
    const sha256 = Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
    invokeMock.mockImplementation((_command, args) => {
      const { offset, length } = args as { offset: number; length: number };
      return Promise.resolve(pdf.slice(offset, offset + length).buffer);
    });
    const builds = createTauriLatexBuilds();
    const artifact = { artifactId: 'a1', jobId: 'j', manifestHash: 'm', label: 'main.pdf', bytes: total, sha256, pages: 1, synctex: true, builtAt: 0 };
    const read = await builds.readArtifact('v1', 'p1', artifact);
    if (!read.ok) throw new Error('expected the bytes');
    expect(read.value.length).toBe(total);
    expect(read.value.every((byte, index) => byte === pdf[index])).toBe(true);
    expect(invokeMock.mock.calls.map(([, args]) => (args as { length: number }).length)).toEqual([ARTIFACT_CHUNK_BYTES, 1000]);

    const tampered = await builds.readArtifact('v1', 'p1', { ...artifact, sha256: '0'.repeat(64) });
    expect(tampered.ok).toBe(false);
    if (tampered.ok) throw new Error('expected a failure');
    expect(tampered.error.messageKey).toBe('error.buildArtifactChanged');
  });

  it('fails visibly outside the desktop host', async () => {
    const outcome = await disconnectedLatexService.listProjects('v1');
    expect(outcome.ok).toBe(false);
    if (outcome.ok) throw new Error('expected a failure');
    expect(outcome.error.messageKey).toBe('error.latexProjectsUnavailable');
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('turns a host conflict into a typed outcome instead of throwing', async () => {
    const service = createTauriLatexService();
    invokeMock.mockRejectedValueOnce({ code: 'VERSION_CONFLICT', messageKey: 'error.projectFileChanged', params: {} });
    const outcome = await service.saveProjectFile('v1', 'p1', 'f1', 'x', 1, 'op');
    expect(outcome.ok).toBe(false);
    if (outcome.ok) throw new Error('expected a failure');
    expect(outcome.error.code).toBe('VERSION_CONFLICT');
    expect(outcome.error.messageKey).toBe('error.projectFileChanged');
  });
});
