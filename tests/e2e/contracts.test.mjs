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
  const dindBlock = /^  dind:\s*$([\s\S]*?)^  target:\s*$/m.exec(compose)?.[1] || '';
  assert.notEqual(dindBlock, '', 'dind service block must be present');
  assert.match(dindBlock, /command:\s*\["dockerd", "--host=tcp:\/\/127\.0\.0\.1:2375"\]/);
  assert.doesNotMatch(dindBlock, /0\.0\.0\.0:2375/);
});

test('Compose service names match the launcher contract', async () => {
  const compose = await read('tests/e2e/compose.yaml');
  for (const service of ['keygen', 'dind', 'target', 'git-fixture', 'client']) {
    assert.match(compose, new RegExp(`^  ${service}:`, 'm'));
  }
});

test('scenario drives deploy, redeploy, and remove through the shared library', async () => {
  const [scenario, lib] = await Promise.all([
    read('tests/e2e/scenario.sh'),
    read('tests/e2e/lib.sh'),
  ]);
  assert.match(scenario, /^source \/opt\/e2e\/lib\.sh$/m);
  assert.match(scenario, /^e2e_bootstrap$/m);
  for (const helper of [
    'fail()',
    'e2e_client_init()',
    'e2e_bootstrap()',
    'run_capture()',
    'assert_log()',
    'assert_deploy_log()',
  ]) {
    assert.ok(lib.includes(helper), `lib.sh defines ${helper}`);
  }
  assert.match(lib, /unset PI_AGENT_URL/);
  assert.match(lib, /ssh-keyscan -H target/);
  assert.match(lib, /\/etc\/ssh\/ssh_known_hosts/);
  assert.doesNotMatch(scenario, /PI_AGENT_URL=/);
  assert.doesNotMatch(scenario, /\$HOME\/\.ssh/);
  assert.equal((scenario.match(/rpi deploy/g) || []).length, 2);
  assert.match(scenario, /rpi ls/);
  assert.match(scenario, /127\.0\.0\.1:18080\/health/);
  assert.match(scenario, /rpi rm e2e-fixture --yes/);
  assert.match(scenario, /com\.docker\.compose\.project=e2e-fixture/);
  assert.match(scenario, /env DOCKER_HOST=tcp:\/\/127\.0\.0\.1:2375 docker ps/);
  for (const milestone of [
    'fetched ',
    'docker compose build ...',
    'docker compose up -d ...',
    'healthcheck: passed',
  ]) {
    assert.match(lib, new RegExp(milestone.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  }
});

test('dev profile provides an exec-able client and stays out of the CI path', async () => {
  const compose = await read('tests/e2e/compose.yaml');
  assert.match(compose, /^  client-dev:/m);
  const devBlock = /^  client-dev:\s*$([\s\S]*?)^networks:/m.exec(compose)?.[1] || '';
  assert.match(devBlock, /profiles: \["dev"\]/);
  assert.match(devBlock, /command: \["sleep", "infinity"\]/);
  assert.match(devBlock, /init: true/);
  const workflow = await read('.github/workflows/ci.yml');
  assert.doesNotMatch(workflow, /RPI_E2E_KEEP|client-dev|--profile dev/);
  const pkg = await read('package.json');
  assert.match(pkg, /"e2e:dev:up": "node tests\/e2e\/run\.mjs --dev-up"/);
  assert.match(pkg, /"e2e:dev:down": "node tests\/e2e\/run\.mjs --dev-down"/);
});

test('CI runs the e2e gate with Buildx cache and failure-only artifacts', async () => {
  const workflow = await read('.github/workflows/ci.yml');
  assert.match(workflow, /branches: \[master\]/);
  assert.match(workflow, /^  pull_request:\s*$/m);
  assert.match(workflow, /permissions:\s*\n  contents: read/);
  assert.match(workflow, /^  e2e:\s*$/m);
  assert.match(workflow, /needs: linux/);
  assert.match(workflow, /runs-on: ubuntu-latest/);
  assert.match(workflow, /timeout-minutes: 30/);
  assert.match(workflow, /docker\/setup-buildx-action@v4/);
  assert.match(workflow, /docker\/build-push-action@v7/);
  assert.match(workflow, /cache-from: type=gha,scope=rpi-e2e/);
  assert.match(workflow, /cache-to: type=gha,mode=max,scope=rpi-e2e,ignore-error=true/);
  assert.match(workflow, /RPI_E2E_PREBUILT: "1"/);
  assert.match(workflow, /npm run test:e2e/);
  assert.match(workflow, /if: failure\(\)/);
  assert.match(workflow, /actions\/upload-artifact@v7/);
  assert.doesNotMatch(workflow, /runs-on: self-hosted/);
});
