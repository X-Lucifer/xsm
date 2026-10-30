'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { createHash } = require('node:crypto');
const publishRelease = require('../publish-release.cjs');

const VERSION = '1.0.0';
const COMMIT = 'a'.repeat(40);
const OTHER_COMMIT = 'b'.repeat(40);
const TAG = `v${VERSION}-build.42`;
const ZIP = `xsm-${VERSION}-windows-x64.zip`;
const hash = (value) => createHash('sha256').update(value).digest('hex');
const httpError = (status) => Object.assign(new Error(`HTTP ${status}`), { status });

async function fixture(t) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'xsm-release-test-'));
  t.after(async () => {
    assert.equal(path.dirname(path.resolve(directory)), path.resolve(os.tmpdir()));
    assert.match(path.basename(directory), /^xsm-release-test-/);
    await fs.rm(directory, { recursive: true, force: true });
  });
  const files = new Map([
    ['xsm.exe', Buffer.from('MZ fixture executable')],
    [ZIP, Buffer.from('PK fixture archive')],
  ]);
  files.set('SHA256SUMS.txt', Buffer.from(
    [...files].map(([name, data]) => `${hash(data)}  ${name}`).join('\n') + '\n',
  ));
  for (const [name, data] of files) await fs.writeFile(path.join(directory, name), data);
  return { directory, files };
}

function mock(files, options = {}) {
  const calls = [];
  const state = {
    release: options.release ? {
      id: 10,
      tag_name: TAG,
      target_commitish: options.target ?? COMMIT,
      draft: options.release === 'draft',
      body: options.body ?? `<!-- xsm-run:42:${COMMIT} -->`,
      html_url: `https://github.com/owner/xsm/releases/tag/${TAG}`,
    } : null,
    reference: options.reference ?? (options.release === 'published'
      ? { type: 'commit', sha: COMMIT } : null),
    assets: [],
    bytes: new Map(),
  };
  function addAsset(name, data = files.get(name)) {
    const asset = {
      id: 100 + state.assets.length,
      name,
      state: 'uploaded',
      size: data.length,
      digest: options.noDigest ? null : `sha256:${hash(data)}`,
      content_type: name.endsWith('.zip') ? 'application/zip'
        : name.endsWith('.txt') ? 'text/plain' : 'application/octet-stream',
    };
    state.assets.push(asset);
    state.bytes.set(asset.id, Buffer.from(data));
    return asset;
  }
  for (const name of options.assets ?? []) addAsset(name);
  const endpoint = (name, fn) => async (args) => {
    calls.push({ name, args });
    if (options.errors?.[name]) throw options.errors[name];
    return { data: await fn(args) };
  };
  const github = { rest: {
    git: {
      getRef: endpoint('getRef', () => {
        if (!state.reference) throw httpError(404);
        return { object: { ...state.reference } };
      }),
      getTag: endpoint('getTag', ({ tag_sha: tagSha }) => {
        const object = options.tags?.[tagSha];
        if (!object) throw httpError(404);
        return { object: { ...object } };
      }),
    },
    repos: {
      get: endpoint('get', () => ({ default_branch: 'main' })),
      getReleaseByTag: endpoint('getReleaseByTag', () => {
        if (!state.release || state.release.draft) throw httpError(404);
        return { ...state.release };
      }),
      listReleases: endpoint('listReleases', ({ page }) => {
        if (options.draftOnSecondPage && page === 1) {
          return Array.from({ length: 100 }, (_, index) => ({ id: index + 20, tag_name: `old-${index}` }));
        }
        return state.release ? [{ ...state.release }] : [];
      }),
      createRelease: endpoint('createRelease', (args) => {
        assert.equal(state.release, null, 'must not create a second release');
        state.release = { ...args, id: 10, html_url: `https://github.com/owner/xsm/releases/tag/${TAG}` };
        return { ...state.release };
      }),
      getRelease: endpoint('getRelease', () => ({ ...state.release })),
      listReleaseAssets: endpoint('listReleaseAssets', () => state.assets.map((asset) => ({ ...asset }))),
      uploadReleaseAsset: endpoint('uploadReleaseAsset', ({ name, data, headers }) => {
        assert.equal(state.release.draft, true, 'assets must only be uploaded to a draft');
        assert.equal(state.assets.some((asset) => asset.name === name), false, 'must not replace an asset');
        const asset = addAsset(name, data);
        assert.equal(headers['content-type'], asset.content_type);
        assert.equal(headers['content-length'], data.length);
        if (options.corruptUpload) asset.digest = `sha256:${'0'.repeat(64)}`;
        return { ...asset };
      }),
      getReleaseAsset: endpoint('getReleaseAsset', ({ asset_id: id, headers }) => {
        assert.equal(headers.accept, 'application/octet-stream');
        if (options.textManifest && state.assets.find((asset) => asset.id === id)?.name === 'SHA256SUMS.txt') {
          return state.bytes.get(id).toString('utf8');
        }
        return options.metadataInsteadOfBinary ? { name: 'unexpected JSON' } : state.bytes.get(id);
      }),
      getBranch: endpoint('getBranch', () => ({ commit: { sha: options.head ?? COMMIT } })),
      updateRelease: endpoint('updateRelease', (args) => {
        assert.equal(state.assets.length, files.size, 'cannot publish a partial upload');
        state.release = { ...state.release, ...args };
        state.reference ??= { type: 'commit', sha: state.release.target_commitish };
        return { ...state.release };
      }),
    },
  } };
  const context = {
    eventName: 'push',
    repo: { owner: 'owner', repo: 'xsm' },
    payload: { repository: { default_branch: 'main' } },
    ref: 'refs/heads/main',
    sha: COMMIT,
    runId: 42,
    runNumber: 7,
  };
  const core = { info() {} };
  return { github, context, core, calls, state };
}

const writes = (api) => api.calls.filter(({ name }) => ['createRelease', 'uploadReleaseAsset', 'updateRelease'].includes(name));
const invoke = (files, api, extra = {}) => publishRelease({
  ...api, artifactDirectory: files.directory, version: VERSION, commit: COMMIT, ...extra,
});

test('fresh publication creates a draft, verifies all uploads, then publishes Latest', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files);
  const result = await invoke(files, api);
  assert.equal(result.tag, TAG);
  assert.equal(result.skipped, false);
  assert.deepEqual(writes(api).map(({ name }) => name), [
    'createRelease', 'uploadReleaseAsset', 'uploadReleaseAsset', 'uploadReleaseAsset', 'updateRelease',
  ]);
  const create = writes(api)[0].args;
  assert.equal(create.tag_name, TAG);
  assert.equal(create.target_commitish, COMMIT);
  assert.equal(create.draft, true);
  assert.equal(create.make_latest, 'false');
  assert.equal(create.name, 'XSM 1.0.0 (build 7)');
  assert.ok(create.body.includes(`<!-- xsm-run:42:${COMMIT} -->`));
  const update = writes(api).at(-1).args;
  assert.equal(update.draft, false);
  assert.equal(update.make_latest, 'true');
  assert.deepEqual(api.state.assets.map(({ name }) => name), [...files.files.keys()]);
});

test('a complete published release is verified and never written or made Latest again', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { release: 'published', assets: [...files.files.keys()], head: OTHER_COMMIT });
  const result = await invoke(files, api);
  assert.equal(result.skipped, true);
  assert.deepEqual(writes(api), []);
  assert.equal(api.calls.some(({ name }) => name === 'getBranch'), false);
});

test('a checksum manifest decoded as text is verified without changing a published release', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { release: 'published', assets: [...files.files.keys()], textManifest: true });
  assert.equal((await invoke(files, api)).skipped, true);
  assert.deepEqual(writes(api), []);
});

test('rerunning a completed build verifies historical checksums even if rebuilt bytes differ', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { release: 'published', assets: [...files.files.keys()] });
  const rebuilt = new Map([
    ['xsm.exe', Buffer.from('MZ rebuilt on a different day')],
    [ZIP, Buffer.from('PK rebuilt with different archive timestamps')],
  ]);
  for (const [name, data] of rebuilt) await fs.writeFile(path.join(files.directory, name), data);
  await fs.writeFile(path.join(files.directory, 'SHA256SUMS.txt'),
    [...rebuilt].map(([name, data]) => `${hash(data)}  ${name}`).join('\n') + '\n');
  assert.equal((await invoke(files, api)).skipped, true);
  assert.deepEqual(writes(api), []);
});

test('a published release must still match its own remote checksum manifest', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { release: 'published', assets: [...files.files.keys()] });
  api.state.assets[0].digest = `sha256:${'0'.repeat(64)}`;
  await assert.rejects(invoke(files, api), /Asset SHA-256 mismatch/);
  assert.deepEqual(writes(api), []);
});

test('a partial draft is found through paginated releases and resumes missing assets only', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { release: 'draft', assets: ['xsm.exe'], draftOnSecondPage: true });
  await invoke(files, api);
  assert.equal(api.calls.filter(({ name }) => name === 'listReleases').length, 2);
  assert.deepEqual(writes(api).map(({ name, args }) => [name, args.name]), [
    ['uploadReleaseAsset', ZIP], ['uploadReleaseAsset', 'SHA256SUMS.txt'], ['updateRelease', undefined],
  ]);
});

test('authorization and network failures never masquerade as a missing release', async (t) => {
  const files = await fixture(t);
  for (const [endpoint, error] of [
    ['getReleaseByTag', httpError(401)],
    ['getReleaseByTag', httpError(403)],
    ['getReleaseByTag', new Error('network disconnected')],
    ['listReleases', httpError(404)],
    ['getRef', httpError(401)],
  ]) {
    const api = mock(files.files, { errors: { [endpoint]: error } });
    await assert.rejects(invoke(files, api), (actual) => actual === error);
    assert.deepEqual(writes(api), []);
  }
});

test('an existing tag pointing to a different commit prevents all writes', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { reference: { type: 'commit', sha: OTHER_COMMIT } });
  await assert.rejects(invoke(files, api), /different commit/);
  assert.deepEqual(writes(api), []);
});

test('annotated tags are dereferenced to their actual commit', async (t) => {
  const files = await fixture(t);
  for (const actual of [COMMIT, OTHER_COMMIT]) {
    const first = 'c'.repeat(40);
    const nested = 'd'.repeat(40);
    const api = mock(files.files, {
      release: 'published', assets: [...files.files.keys()],
      reference: { type: 'tag', sha: first },
      tags: { [first]: { type: 'tag', sha: nested }, [nested]: { type: 'commit', sha: actual } },
    });
    if (actual === COMMIT) assert.equal((await invoke(files, api)).skipped, true);
    else await assert.rejects(invoke(files, api), /different commit/);
    assert.equal(api.calls.filter(({ name }) => name === 'getTag').length, 2);
    assert.deepEqual(writes(api), []);
  }
});

test('a tagless draft must be pinned to this exact commit', async (t) => {
  const files = await fixture(t);
  for (const target of [OTHER_COMMIT, 'main']) {
    const api = mock(files.files, { release: 'draft', target });
    await assert.rejects(invoke(files, api), /pinned draft commit/);
    assert.deepEqual(writes(api), []);
  }
});

test('a manual or different-run draft is never resumed', async (t) => {
  const files = await fixture(t);
  for (const body of ['', 'Handwritten release', `<!-- xsm-run:41:${COMMIT} -->`, `<!-- xsm-run:42:${OTHER_COMMIT} -->`]) {
    const api = mock(files.files, { release: 'draft', body });
    await assert.rejects(invoke(files, api), /not marked for this workflow run/);
    assert.deepEqual(writes(api), []);
  }
});

test('existing asset mismatches fail before uploading any missing files', async (t) => {
  const files = await fixture(t);
  for (const change of [
    { digest: `sha256:${'0'.repeat(64)}` }, { size: 1 }, { state: 'starter' }, { content_type: 'text/html' },
  ]) {
    const api = mock(files.files, { release: 'draft', assets: ['xsm.exe'] });
    Object.assign(api.state.assets[0], change);
    await assert.rejects(invoke(files, api), /mismatch/);
    assert.deepEqual(writes(api), []);
  }
});

test('incomplete published releases are not repaired in place', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { release: 'published', assets: ['xsm.exe'] });
  await assert.rejects(invoke(files, api), /incomplete/);
  assert.deepEqual(writes(api), []);
});

test('older commits publish without taking Latest from the default branch head', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { head: OTHER_COMMIT });
  await invoke(files, api);
  assert.equal(writes(api).at(-1).args.make_latest, 'false');
});

test('missing server digests are verified by downloading binary assets', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { release: 'published', assets: [...files.files.keys()], noDigest: true });
  assert.equal((await invoke(files, api)).skipped, true);
  assert.equal(api.calls.filter(({ name }) => name === 'getReleaseAsset').length, 3);
  assert.deepEqual(writes(api), []);
});

test('unverifiable or corrupt downloaded assets block draft publication', async (t) => {
  const files = await fixture(t);
  for (const bad of ['json', 'bytes', 'permissions']) {
    const api = mock(files.files, {
      release: 'draft', assets: ['xsm.exe'], noDigest: true,
      metadataInsteadOfBinary: bad === 'json',
      errors: bad === 'permissions' ? { getReleaseAsset: httpError(403) } : undefined,
    });
    if (bad === 'bytes') api.state.bytes.set(api.state.assets[0].id, Buffer.from('corrupt'));
    await assert.rejects(invoke(files, api), /binary asset content|SHA-256 mismatch|HTTP 403/);
    assert.deepEqual(writes(api), []);
  }
});

test('upload verification failure leaves a draft rather than publishing partial assets', async (t) => {
  const files = await fixture(t);
  const api = mock(files.files, { corruptUpload: true });
  await assert.rejects(invoke(files, api), /Asset SHA-256 mismatch/);
  assert.equal(api.state.release.draft, true);
  assert.equal(api.calls.some(({ name }) => name === 'updateRelease'), false);
});

test('checksum coverage, duplicate names and path traversal are rejected before REST calls', async (t) => {
  const files = await fixture(t);
  const valid = files.files.get('SHA256SUMS.txt').toString();
  for (const checksums of [
    valid.split('\n')[0] + '\n',
    valid + valid.split('\n')[0] + '\n',
    valid.replace('xsm.exe', '../xsm.exe'),
    valid.replace('xsm.exe', 'C:\\outside\\xsm.exe'),
    valid.replace(/^[a-f0-9]{64}/, '0'.repeat(64)),
  ]) {
    await fs.writeFile(path.join(files.directory, 'SHA256SUMS.txt'), checksums);
    const api = mock(files.files);
    await assert.rejects(invoke(files, api), /checksum|SHA256SUMS/);
    assert.deepEqual(api.calls, []);
  }
});

test('multiple x64 archives or a version mismatch are not guessed', async (t) => {
  const files = await fixture(t);
  await fs.writeFile(path.join(files.directory, 'xsm-0.9.0-windows-x64.zip'), 'other archive');
  const api = mock(files.files);
  await assert.rejects(invoke(files, api), /exactly one x64 archive/);
  assert.deepEqual(api.calls, []);
});

test('PRs, non-default pushes, tag pushes and branch deletion cannot publish', async (t) => {
  const files = await fixture(t);
  for (const patch of [
    { eventName: 'pull_request' }, { ref: 'refs/heads/feature' }, { ref: 'refs/tags/v1.0.0' },
    { payload: { repository: { default_branch: 'main' }, deleted: true } },
  ]) {
    const api = mock(files.files);
    Object.assign(api.context, patch);
    assert.equal((await invoke(files, api)).skipped, true);
    assert.deepEqual(api.calls, []);
  }
});

test('commit provenance and unsafe version names are rejected before REST calls', async (t) => {
  const files = await fixture(t);
  for (const args of [{ commit: OTHER_COMMIT }, { version: '../1.0.0' }]) {
    const api = mock(files.files);
    await assert.rejects(invoke(files, api, args), /triggering push SHA|Invalid package version/);
    assert.deepEqual(api.calls, []);
  }
});
