'use strict';

const fs = require('node:fs/promises');
const path = require('node:path');
const { createHash } = require('node:crypto');

const sha256 = (data) => createHash('sha256').update(data).digest('hex');

function ensure(condition, message) {
  if (!condition) throw new Error(message);
}

async function optionalGet(request) {
  try {
    return (await request()).data;
  } catch (error) {
    if (error.status === 404) return null;
    throw error;
  }
}

async function allPages(request) {
  const results = [];
  for (let page = 1; ; page += 1) {
    const { data } = await request({ per_page: 100, page });
    ensure(Array.isArray(data), 'GitHub returned an invalid paginated response');
    results.push(...data);
    if (data.length < 100) return results;
  }
}

async function readArtifacts(directory, version) {
  const root = path.resolve(directory);
  ensure((await fs.lstat(root)).isDirectory(), 'Artifact directory must be a real directory');
  const archive = `xsm-${version}-windows-x64.zip`;
  const archives = (await fs.readdir(root)).filter((name) => /^xsm-.*-windows-x64\.zip$/.test(name));
  ensure(archives.length === 1 && archives[0] === archive, `Expected exactly one x64 archive named ${archive}`);
  const names = ['xsm.exe', archive, 'SHA256SUMS.txt'];
  const artifacts = [];
  for (const name of names) {
    const file = path.join(root, name);
    ensure((await fs.lstat(file)).isFile(), `Release asset must be a regular file: ${name}`);
    const data = await fs.readFile(file);
    ensure(data.length > 0, `Release asset is empty: ${name}`);
    artifacts.push({
      name,
      data,
      digest: sha256(data),
      contentType: name.endsWith('.zip') ? 'application/zip'
        : name.endsWith('.txt') ? 'text/plain' : 'application/octet-stream',
    });
  }

  const checksums = parseChecksums(artifacts[2].data, names.slice(0, 2));
  for (const artifact of artifacts.slice(0, 2)) {
    ensure(checksums.get(artifact.name) === artifact.digest, `Local checksum mismatch: ${artifact.name}`);
  }
  return artifacts;
}

function parseChecksums(data, names) {
  const checksums = new Map();
  for (const line of data.toString('utf8').split(/\r?\n/).filter((line) => line.length > 0)) {
    const match = /^([a-fA-F0-9]{64}) [ *](.+)$/.exec(line);
    ensure(match, 'Invalid SHA256SUMS.txt line; expected SHA-256 and a fixed asset filename');
    const [, hash, name] = match;
    ensure(names.includes(name), `Unexpected checksum entry: ${name}`);
    ensure(!checksums.has(name), `Duplicate checksum entry: ${name}`);
    checksums.set(name, hash.toLowerCase());
  }
  ensure(checksums.size === 2, 'SHA256SUMS.txt must cover exactly xsm.exe and the x64 ZIP');
  return checksums;
}

async function findRelease(github, repository, tag) {
  const published = await optionalGet(() => github.rest.repos.getReleaseByTag({ ...repository, tag }));
  if (published) return published;
  // getReleaseByTag only guarantees published releases. Listing with contents
  // write permission also finds a draft left by an interrupted upload.
  const matches = (await allPages((paging) => github.rest.repos.listReleases({ ...repository, ...paging })))
    .filter((release) => release.tag_name === tag);
  ensure(matches.length <= 1, `Multiple releases use ${tag}; refusing to choose or overwrite one`);
  return matches[0] ?? null;
}

async function tagCommit(github, repository, tag) {
  const reference = await optionalGet(() => github.rest.git.getRef({ ...repository, ref: `tags/${tag}` }));
  if (!reference) return null;
  let object = reference.object;
  const visited = new Set();
  while (object?.type === 'tag') {
    ensure(/^[a-fA-F0-9]{40}$/.test(object.sha), `Invalid annotated tag object for ${tag}`);
    ensure(!visited.has(object.sha) && visited.size < 32, `Cyclic or excessively nested annotated tag: ${tag}`);
    visited.add(object.sha);
    object = (await github.rest.git.getTag({ ...repository, tag_sha: object.sha })).data.object;
  }
  ensure(object?.type === 'commit' && /^[a-fA-F0-9]{40}$/.test(object.sha), `Tag ${tag} does not resolve to a commit`);
  return object.sha.toLowerCase();
}

async function verifyIdentity(github, repository, release, tag, commit) {
  const actualCommit = await tagCommit(github, repository, tag);
  ensure(actualCommit === null || actualCommit === commit, `Tag ${tag} points to a different commit; refusing to overwrite it`);
  if (!release) return;
  ensure(release.tag_name === tag && Number.isSafeInteger(release.id), `Invalid release identity for ${tag}`);
  ensure(typeof release.draft === 'boolean', `Invalid draft state for ${tag}`);
  if (actualCommit === null) {
    ensure(release.draft && release.target_commitish?.toLowerCase() === commit,
      `Release ${tag} has no matching tag or pinned draft commit; refusing to modify it`);
  }
}

async function downloadAsset(github, repository, asset) {
  const { data } = await github.rest.repos.getReleaseAsset({
    ...repository,
    asset_id: asset.id,
    headers: { accept: 'application/octet-stream' },
  });
  let bytes;
  if (Buffer.isBuffer(data)) bytes = data;
  else if (data instanceof ArrayBuffer) bytes = Buffer.from(data);
  else if (ArrayBuffer.isView(data)) bytes = Buffer.from(data.buffer, data.byteOffset, data.byteLength);
  else if (typeof data === 'string' && asset.name === 'SHA256SUMS.txt') bytes = Buffer.from(data, 'utf8');
  ensure(bytes, `Cannot verify ${asset.name}: GitHub did not return binary asset content`);
  return bytes;
}

async function verifyAsset(github, repository, asset, expected, downloaded) {
  const size = expected.size ?? expected.data.length;
  ensure(asset.name === expected.name && Number.isSafeInteger(asset.id), `Invalid asset identity: ${expected.name}`);
  ensure(asset.state === 'uploaded' && Number.isSafeInteger(asset.size) && asset.size > 0 && asset.size === size,
    `Asset state or size mismatch: ${expected.name}`);
  ensure(asset.content_type?.split(';')[0].trim().toLowerCase() === expected.contentType,
    `Asset content type mismatch: ${expected.name}`);
  if (asset.digest != null && asset.digest !== '') {
    ensure(asset.digest.toLowerCase() === `sha256:${expected.digest}`, `Asset SHA-256 mismatch: ${expected.name}`);
    return;
  }
  const bytes = downloaded ?? await downloadAsset(github, repository, asset);
  ensure(bytes.length === size && sha256(bytes) === expected.digest,
    `Downloaded asset SHA-256 mismatch: ${expected.name}`);
}

async function releaseAssets(github, repository, release, artifacts) {
  const assets = await allPages((paging) => github.rest.repos.listReleaseAssets({
    ...repository, release_id: release.id, ...paging,
  }));
  const byName = new Map();
  for (const asset of assets) {
    ensure(!byName.has(asset.name), `Duplicate release asset: ${asset.name}`);
    ensure(artifacts.some((expected) => expected.name === asset.name), `Unexpected release asset: ${asset.name}`);
    byName.set(asset.name, asset);
  }
  return byName;
}

async function verifiedMissingAssets(github, repository, release, artifacts) {
  const byName = await releaseAssets(github, repository, release, artifacts);
  const missing = [];
  for (const expected of artifacts) {
    const asset = byName.get(expected.name);
    if (asset) await verifyAsset(github, repository, asset, expected);
    else missing.push(expected);
  }
  return missing;
}

async function verifyPublishedAssets(github, repository, release, artifacts) {
  const byName = await releaseAssets(github, repository, release, artifacts);
  ensure(artifacts.every(({ name }) => byName.has(name)),
    `Published release ${release.tag_name} is incomplete; published releases are never modified`);
  // A rerun can rebuild different ZIP timestamps or executable build dates.
  // Verify history against its own checksum manifest, not the new local build.
  const manifest = byName.get('SHA256SUMS.txt');
  const data = await downloadAsset(github, repository, manifest);
  await verifyAsset(github, repository, manifest, {
    name: manifest.name, data, digest: sha256(data), contentType: 'text/plain',
  }, data);
  const checksums = parseChecksums(data, artifacts.slice(0, 2).map(({ name }) => name));
  for (const expected of artifacts.slice(0, 2)) {
    const asset = byName.get(expected.name);
    await verifyAsset(github, repository, asset, {
      name: expected.name, contentType: expected.contentType, size: asset.size,
      digest: checksums.get(expected.name),
    });
  }
}

function verifyDraftMarker(release, marker) {
  if (release.draft) {
    ensure(typeof release.body === 'string' && release.body.includes(marker),
      `Draft ${release.tag_name} is not marked for this workflow run and commit; refusing to modify it`);
  }
}

module.exports = async function publishRelease({ github, context, core, artifactDirectory, version, commit }) {
  if (context.eventName !== 'push') {
    core.info('Release publication is only enabled for default-branch pushes');
    return { skipped: true, reason: 'not-push' };
  }
  if (context.payload?.deleted === true) {
    core.info('Skipping release publication for a deleted branch');
    return { skipped: true, reason: 'deleted-branch' };
  }
  const repository = context.repo;
  ensure(repository?.owner && repository.repo, 'Repository owner and name are required');
  const defaultBranch = context.payload?.repository?.default_branch
    ?? (await github.rest.repos.get(repository)).data.default_branch;
  ensure(typeof defaultBranch === 'string' && defaultBranch.length > 0, 'Cannot determine the default branch');
  if (context.ref !== `refs/heads/${defaultBranch}`) {
    core.info('Skipping release publication outside the default branch');
    return { skipped: true, reason: 'not-default-branch' };
  }
  ensure(/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(version),
    'Invalid package version');
  ensure(typeof commit === 'string' && /^[a-fA-F0-9]{40}$/.test(commit), 'A full commit SHA is required');
  commit = commit.toLowerCase();
  ensure(typeof context.sha === 'string' && context.sha.toLowerCase() === commit, 'Release commit must match the triggering push SHA');
  const runId = String(context.runId);
  ensure(/^[1-9]\d*$/.test(runId), 'A positive workflow run ID is required');
  const tag = `v${version}-build.${runId}`;
  const marker = `<!-- xsm-run:${runId}:${commit} -->`;
  const artifacts = await readArtifacts(artifactDirectory, version);
  let release = await findRelease(github, repository, tag);
  await verifyIdentity(github, repository, release, tag, commit);
  if (release) verifyDraftMarker(release, marker);
  if (!release) {
    const runNumber = context.runNumber == null ? runId : String(context.runNumber);
    ensure(/^[1-9]\d*$/.test(runNumber), 'A positive workflow run number is required');
    release = (await github.rest.repos.createRelease({
      ...repository,
      tag_name: tag,
      target_commitish: commit,
      name: `XSM ${version} (build ${runNumber})`,
      body: `Windows x64 build from commit ${commit}.\n\nWorkflow run: ${runId}.\n\n${marker}`,
      draft: true,
      prerelease: false,
      make_latest: 'false',
    })).data;
    ensure(release.draft === true, 'GitHub did not create a draft release; refusing to upload assets');
    await verifyIdentity(github, repository, release, tag, commit);
    verifyDraftMarker(release, marker);
  }

  if (!release.draft) {
    await verifyPublishedAssets(github, repository, release, artifacts);
    core.info(`Release ${tag} already contains the verified assets; leaving it unchanged`);
    return { tag, releaseId: release.id, url: release.html_url, skipped: true };
  }
  const missing = await verifiedMissingAssets(github, repository, release, artifacts);
  for (const artifact of missing) {
    const uploaded = (await github.rest.repos.uploadReleaseAsset({
      ...repository,
      release_id: release.id,
      name: artifact.name,
      headers: { 'content-type': artifact.contentType, 'content-length': artifact.data.length },
      data: artifact.data,
    })).data;
    await verifyAsset(github, repository, uploaded, artifact);
  }

  // Re-read the draft and all uploaded assets before the only publication call.
  release = (await github.rest.repos.getRelease({ ...repository, release_id: release.id })).data;
  await verifyIdentity(github, repository, release, tag, commit);
  if (!release.draft) {
    await verifyPublishedAssets(github, repository, release, artifacts);
    core.info(`Release ${tag} was already published; leaving its Latest status unchanged`);
    return { tag, releaseId: release.id, url: release.html_url, skipped: true };
  }
  verifyDraftMarker(release, marker);
  ensure((await verifiedMissingAssets(github, repository, release, artifacts)).length === 0,
    `Release ${tag} is missing assets after upload; leaving the draft unpublished`);
  const head = (await github.rest.repos.getBranch({ ...repository, branch: defaultBranch })).data.commit?.sha;
  ensure(typeof head === 'string' && /^[a-fA-F0-9]{40}$/.test(head), 'Cannot verify the current default-branch head');
  const published = (await github.rest.repos.updateRelease({
    ...repository,
    release_id: release.id,
    draft: false,
    prerelease: false,
    make_latest: head.toLowerCase() === commit ? 'true' : 'false',
  })).data;
  ensure(published.draft === false, `Release ${tag} was not published`);
  await verifyIdentity(github, repository, published, tag, commit);
  core.info(`Published ${tag}`);
  return { tag, releaseId: published.id, url: published.html_url, skipped: false };
};
