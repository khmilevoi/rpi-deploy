import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');
const read = (relative) => readFile(path.join(ROOT, relative), 'utf8');

test('fixture uses local Git, managed port allocation, HTTP fallback, and LF content', async () => {
  const [attributes, config, compose, dockerfile, health] = await Promise.all([
    read('.gitattributes'),
    read('tests/e2e/fixtures/app/rpi.toml'),
    read('tests/e2e/fixtures/app/compose.yaml'),
    read('tests/e2e/fixtures/app/Dockerfile'),
    read('tests/e2e/fixtures/app/health'),
  ]);
  assert.match(attributes, /tests\/e2e\/\*\.sh text eol=lf/);
  assert.match(attributes, /tests\/e2e\/fixtures\/app\/health text eol=lf/);
  assert.match(config, /name = "e2e-fixture"/);
  assert.match(config, /repo = "git:\/\/git-fixture\/fixture\.git"/);
  assert.match(config, /service = "web"/);
  assert.match(config, /port = 8080/);
  assert.match(config, /path = "\/health"/);
  assert.match(config, /expect = "200"/);
  assert.doesNotMatch(compose, /^\s*ports:/m);
  assert.match(compose, /^\s*expose:/m);
  assert.doesNotMatch(dockerfile, /^HEALTHCHECK/m);
  assert.equal(health.trim(), 'ok');
});

test('runtime builds one current rpi binary and contains required target tools', async () => {
  const [dockerfile, agent, target, git] = await Promise.all([
    read('tests/e2e/Dockerfile'),
    read('tests/e2e/agent.toml'),
    read('tests/e2e/target-entrypoint.sh'),
    read('tests/e2e/git-entrypoint.sh'),
  ]);
  assert.match(dockerfile, /cargo build --locked -p pi/);
  assert.match(dockerfile, /COPY --from=builder \/out\/rpi \/usr\/local\/bin\/rpi/);
  assert.match(dockerfile, /FROM docker:28-cli AS docker_cli/);
  assert.match(dockerfile, /docker-compose/);
  assert.match(agent, /socket = "\/run\/rpi\/agent\.sock"/);
  assert.match(agent, /port_min = 18080/);
  assert.match(agent, /port_max = 18089/);
  assert.match(target, /runuser -u rpi-agent/);
  assert.match(target, /AllowStreamLocalForwarding=yes/);
  assert.match(git, /git daemon/);
  assert.match(git, /fixture\.git/);
});

test('outer Compose isolates DinD and preserves the target loopback model', async () => {
  const compose = await read('tests/e2e/compose.yaml');
  assert.match(compose, /privileged: true/);
  assert.equal((compose.match(/privileged: true/g) || []).length, 1);
  assert.match(compose, /127\.0\.0\.1:2375/);
  assert.match(compose, /network_mode: service:dind/);
  assert.match(compose, /aliases:\s*\n\s*- target/);
  assert.match(compose, /condition: service_completed_successfully/);
  assert.match(compose, /condition: service_healthy/);
  assert.doesNotMatch(compose, /\/var\/run\/docker\.sock/);
  assert.doesNotMatch(compose, /^\s{4}ports:/m);
  const targetBlock = /^  target:\s*$([\s\S]*?)^  git-fixture:\s*$/m.exec(compose)?.[1] || '';
  assert.match(targetBlock, /ssh-public:\/run\/e2e-public:ro/);
  assert.doesNotMatch(targetBlock, /ssh-private/);
});

test('Compose service names match the launcher contract', async () => {
  const compose = await read('tests/e2e/compose.yaml');
  for (const service of ['keygen', 'dind', 'target', 'git-fixture', 'client']) {
    assert.match(compose, new RegExp(`^  ${service}:`, 'm'));
  }
});
