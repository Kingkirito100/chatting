# Oracle Cloud Always Free (24/7, без засыпания)

Самый честный бесплатный вариант: виртуальная машина, которая работает
круглосуточно и не засыпает. Карта нужна только для проверки личности,
деньги не списываются.

## 1. Создать машину

1. Регистрация на [cloud.oracle.com](https://cloud.oracle.com) (Always Free).
2. **Compute → Instances → Create instance**.
3. Образ: **Ubuntu 24.04** (или 22.04).
4. Форма: `VM.Standard.E2.1.Micro` (AMD, всегда бесплатно) или
   `VM.Standard.A1.Flex` (ARM, 1 OCPU / 6 ГБ — тоже бесплатно).
5. Скачай SSH-ключ, создай машину, запомни публичный IP.

## 2. Открыть порты

В **Virtual Cloud Network → Security Lists → Default Security List** добавь
Ingress Rules:

| Источник | Протокол | Порт |
|---|---|---|
| `0.0.0.0/0` | TCP | 7777 |
| `0.0.0.0/0` | TCP | 8080 |

На самой машине:

```bash
sudo ufw allow 7777/tcp
sudo ufw allow 8080/tcp
```

## 3. Поставить Docker

```bash
ssh ubuntu@IP_МАШИНЫ
curl -fsSL https://get.docker.com | sudo sh
sudo usermod -aG docker $USER
newgrp docker
```

## 4. Запустить «Круг»

Собери образ локально и залей на машину:

```bash
# на своём компьютере, из папки krug
docker build -t krug .
docker save krug | gzip > krug.tar.gz
scp krug.tar.gz ubuntu@IP_МАШИНЫ:/tmp/

# на машине
gunzip -c /tmp/krug.tar.gz | docker load
docker run -d --name krug --restart unless-stopped \
  -p 7777:7777 -p 8080:8080 \
  -v krug_data:/data \
  -e KRUG_INVITE='svoi-2026' \
  -e KRUG_NAME='Круг' \
  krug
```

Или без Docker, из исходников:

```bash
sudo apt install -y build-essential curl
curl https://sh.rustup.rs -sSf | sh -s -- -y
source ~/.cargo/env
git clone <репозиторий> krug && cd krug
cargo build --release -p krug-server
nohup ./target/release/krug-server \
  --addr 0.0.0.0:7777 --ws-addr 0.0.0.0:8080 \
  --data /home/ubuntu/krug-data --invite 'svoi-2026' \
  > krug.log 2>&1 &
```

## 5. Подключение

В клиенте укажи `IP_МАШИНЫ:7777` (TCP, быстрее) или
`ws://IP_МАШИНЫ:8080` (WebSocket).

Проверка: `curl http://IP_МАШИНЫ:8080/` → `Круг жив`.

## Обслуживание

```bash
docker logs -f krug        # смотреть логи
docker restart krug        # перезапустить
docker rm -f krug          # удалить
```

Данные лежат в томе `krug_data` и переживают перезапуски. Бэкап:

```bash
docker run --rm -v krug_data:/data -v $PWD:/backup alpine \
  tar czf /backup/krug-backup.tar.gz -C /data .
```

## systemd (если без Docker)

`/etc/systemd/system/krug.service`:

```ini
[Unit]
Description=Krug messenger
After=network.target

[Service]
User=ubuntu
ExecStart=/home/ubuntu/krug/target/release/krug-server --addr 0.0.0.0:7777 --ws-addr 0.0.0.0:8080 --data /home/ubuntu/krug-data --invite svoi-2026
Restart=always

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now krug
```
