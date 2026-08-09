# yth Ilhae 원격 추론 전달 문서

이 문서는 yth의 llama-server를 다른 PC의 Ilhae에서 직접 사용하는 현재 운영 구성을 설명한다.

## 현재 구성

```text
클라이언트 PC의 Ilhae
  -> http://121.137.29.228:8083
  -> yth의 ilhae-runtime-proxy
  -> yth의 llama-server (127.0.0.1:8082)
```

- yth 공인 IP: `121.137.29.228`
- 외부 공개 포트: `8083` (`ilhae-runtime-proxy`)
- llama-server 내부 주소: `127.0.0.1:8082`
- systemd 서비스: `ilhae-runtime-proxy.service`
- 서비스 계정: `yth`
- 인증: `X-Ilhae-Runtime-Token` 공유 토큰
- SSH 터널, nginx, Cloudflare는 추론 경로에서 사용하지 않는다.
- `config.toml`의 `native_runtime` 블록이 실행 파일, 모델, 환경 변수와 전체 llama-server 인자를 정의하는 SSOT다.

현재 공인 IP 접속은 HTTP이므로 토큰, 프롬프트와 응답이 전송 구간에서 암호화되지 않는다. 현재 구성을 인터넷 대상의 전송 보안까지 갖춘 배포로 간주하면 안 된다.

## 1. yth 서버 확인

yth에서 다음을 실행한다.

```bash
sudo systemctl --no-pager status ilhae-runtime-proxy
sudo systemctl is-enabled ilhae-runtime-proxy
sudo ss -ltnp | grep ':8083'
```

정상 상태:

- 서비스가 `active (running)`이다.
- 서비스가 `enabled`다.
- 프록시가 `0.0.0.0:8083`에서 수신한다.
- llama-server는 외부가 아닌 `127.0.0.1:8082`에서 수신한다.

프록시 로그:

```bash
sudo journalctl -u ilhae-runtime-proxy -n 100 --no-pager
```

## 2. 프록시 토큰 전달

현재 방식에서는 yth에 설치된 프록시 토큰을 허가된 클라이언트의 `config.toml`에 한 번 복사해야 한다.

yth에서 토큰 확인:

```bash
sudo sed -n 's/^ILHAE_RUNTIME_PROXY_TOKEN=//p' \
  /etc/default/ilhae-runtime-proxy
```

토큰 취급 규칙:

- Git, 메신저 공개 채널, 이슈와 문서에 토큰 값을 기록하지 않는다.
- 허가된 사용자에게 별도 보안 채널로 전달한다.
- 클라이언트 설정 파일 권한은 `0600`으로 제한한다.
- `/etc/default/ilhae-runtime-proxy`를 덮어쓰면 기존 클라이언트 토큰과 불일치할 수 있다.

## 3. 클라이언트 `~/.ilhae/config.toml`

아래 원격 프로필을 추가한다. `REPLACE_WITH_YTH_RUNTIME_TOKEN`만 yth에서 확인한 실제 토큰으로 교체한다.

```toml
[profile]
active = "qwen3.6-27b-yth"

[profiles."qwen3.6-27b-yth".agent]
engine = "ilhae"
command = "ilhae"

[profiles."qwen3.6-27b-yth".permissions]
approval_preset = "full-access"

[profiles."qwen3.6-27b-yth".native_runtime]
enabled = true
provider = "llama-server"

# 프록시 origin만 적는다. /v1 또는 /health를 붙이지 않는다.
proxy_url = "http://121.137.29.228:8083"
proxy_allow_insecure_http = true
proxy_token = "REPLACE_WITH_YTH_RUNTIME_TOKEN"

# 아래 값과 경로는 모두 클라이언트가 아닌 yth 서버 기준이다.
server_bin = "/home/yth/llama-cpp-turboquant/build-gpu/bin/llama-server"
model_path = "qwen3.6-27b-yth"
chat_template_file = ""
log_file = "/tmp/llama-server.log"
context_window = 16384
startup_timeout_secs = 300

args = [
    "--hf-repo",
    "DavidAU/Qwen3.6-27B-Fable-Fusion-711-Uncensored-Heretic-NM-DAU-NEO-MAX-MTP-GGUF:Q4_K_M",
    "--alias",
    "qwen3.6-27b-yth",
    "--host",
    "127.0.0.1",
    "--port",
    "8082",
    "--device",
    "CUDA0",
    "--main-gpu",
    "0",
    "--fit",
    "off",
    "--n-gpu-layers",
    "0",
    "--jinja",
    "--reasoning",
    "off",
    "--ctx-size",
    "16384",
    "--cache-type-k",
    "turbo4",
    "--cache-type-v",
    "turbo4",
    "--spec-type",
    "ngram-mod",
]

[profiles."qwen3.6-27b-yth".native_runtime.env]
```

설정 파일 권한을 제한한다.

```bash
chmod 600 ~/.ilhae/config.toml
```

`proxy_url` 하나에서 Ilhae가 다음 주소를 자동 파생한다.

- 추론: `http://121.137.29.228:8083/v1`
- 상태: `http://121.137.29.228:8083/health`
- 런타임 제어: `http://121.137.29.228:8083/_ilhae/native-runtime/ensure`

`enabled = true`이면 yth 프록시가 같은 `native_runtime` 설정으로 llama-server를 실행하거나 재사용한다. 이미 별도로 관리 중인 llama-server에 접속만 하려면 `enabled = false`를 사용한다.

## 4. 추론 확인

클라이언트 PC에서 실행한다.

```bash
timeout 360s ilhae exec hi
```

정상 기준:

- 종료 코드가 `0`이다.
- yth 모델의 텍스트 응답이 출력된다.
- yth에서 `journalctl` 또는 프로세스 명령행을 확인하면 위 `args`가 적용되어 있다.

토큰 인증만 별도로 확인하려면 클라이언트에서 실행한다.

```bash
curl -i http://121.137.29.228:8083/health
```

토큰이 없으므로 `401 Unauthorized`가 정상이다. 실제 토큰을 셸 기록에 남기지 않기 위해 인증된 상태 확인은 `ilhae exec hi`를 우선 사용한다.

## 5. 장애 확인

### 연결 거부 또는 시간 초과

- yth 서비스 상태를 확인한다.
- 라우터 포트 포워딩이 외부 `8083`을 yth의 `8083`으로 전달하는지 확인한다.
- yth 방화벽에서 허용된 클라이언트가 `8083/tcp`에 접근 가능한지 확인한다.

### `401 Unauthorized`

- 클라이언트의 `proxy_token`과 `/etc/default/ilhae-runtime-proxy`의 값이 같은지 확인한다.
- 서비스 환경 파일을 바꿨다면 `sudo systemctl restart ilhae-runtime-proxy`를 실행한다.

### HTTP 허용 오류

공인 IP에 현재처럼 HTTP로 접속할 때는 다음 설정이 반드시 필요하다.

```toml
proxy_allow_insecure_http = true
```

### llama-server 실행 실패

- `server_bin`, 모델과 템플릿 경로가 yth에 실제로 존재하는지 확인한다.
- `args`의 장치, 포트와 모델 옵션이 yth 환경에 맞는지 확인한다.
- `/tmp/llama-server.log`와 프록시 journal을 확인한다.

## 6. 서비스 재설치가 필요할 때

yth의 소스 체크아웃에서 debug 바이너리를 빌드하고 설치한다.

```bash
cd /home/yth/codex/codex-rs
cargo build -p codex-ilhae --bin ilhae-runtime-proxy
sudo scripts/install-ilhae-runtime-proxy-service.sh \
  --service-user yth \
  --overwrite \
  --enable \
  --start
```

기존 토큰을 유지하려면 `--overwrite-env`를 추가하지 않는다. 설치기는 기존 `/etc/default/ilhae-runtime-proxy`를 보존한다.

## 7. 운영 경계

- yth가 꺼져 있으면 원격 프로필은 사용할 수 없다.
- yth 재부팅 후 프록시는 systemd에 의해 자동 시작된다.
- SSH는 소스 배포와 관리에만 사용하며 추론 트래픽은 SSH를 통과하지 않는다.
- 이 토큰은 identity-server 로그인 토큰이 아니라 현재 프록시 전용 공유 비밀값이다.
- 향후 페어링이나 자격증명 저장소를 추가하기 전까지는 위 수동 토큰 전달 방식이 현재 확정 동작이다.
