# CI: деплой из GitHub Actions (v0.3)

`pi deploy` готов к CI (§23 v0.3): очередь latest-wins переживает два пуша
подряд без ретраев, поэтапные таймауты не дают джобе зависнуть, а флаги
`--host/--user/--key` не требуют клиентского конфига на раннере.

## Секреты репозитория (Settings -> Secrets -> Actions)

| Secret | Что это |
|---|---|
| `PI_HOST` | хост/IP Pi, доступный раннеру по SSH |
| `PI_USER` | логин-юзер Pi (НЕ сервис-юзер `pi-agent`) |
| `PI_SSH_KEY` | приватный ключ; его pubkey - в `authorized_keys` этого юзера |

## .github/workflows/deploy.yml

```yaml
name: deploy

on:
  push:
    branches: [main]

concurrency:
  group: deploy-production
  cancel-in-progress: true # экономит минуты; очередь агента все равно latest-wins

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      # ... тесты проекта ...

  deploy:
    needs: test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install pi CLI
        # бинарные релизы + install.sh появятся в v0.5; пока - из исходников.
        # Ускорение: actions/cache на ~/.cargo/bin по хешу ревизии тулы.
        run: cargo install --git https://github.com/khmilevoi/pi --locked pi

      - name: Prepare SSH
        run: |
          mkdir -p ~/.ssh
          ssh-keyscan -H "${{ secrets.PI_HOST }}" >> ~/.ssh/known_hosts
          install -m 600 /dev/null ~/.ssh/deploy_key
          printf '%s\n' "${{ secrets.PI_SSH_KEY }}" > ~/.ssh/deploy_key

      - name: Deploy
        run: |
          pi deploy \
            --ref "$GITHUB_SHA" \
            --host "${{ secrets.PI_HOST }}" \
            --user "${{ secrets.PI_USER }}" \
            --key ~/.ssh/deploy_key
```

## Почему именно так

- **`--ref "$GITHUB_SHA"`** - деплоится ровно протестированный коммит, а не
  "main на момент деплоя".
- **`ssh-keyscan` обязателен**: SSH-туннель ходит с `BatchMode=yes` - без
  known_hosts соединение упадет на проверке host key.
- **Два пуша подряд**: деплой первого может завершиться `superseded` - CLI
  выходит с кодом 0 и джоба остается зеленой (latest wins, §8.1). Красные
  статусы: `failed`, `canceled`, `interrupted`.
- **Секреты проекта** не шлются из CI на каждый деплой: bundle уже хранится на
  Pi (`pi env send` делается вручную при смене значений, §10).
- **Зависший build** убьет поэтапный таймаут агента (`timeout: build`, дефолт
  30 минут) - джоба упадет с понятной причиной, а не по таймауту раннера.
- **Отмена из CI не нужна**: новый пуш сам вытеснит ожидающий деплой; для
  ручной отмены с рабочей машины есть `pi deploy --cancel`.
