const conversation = document.querySelector("#conversation");
const composer = document.querySelector("#composer");
const input = document.querySelector("#messageInput");
const eventList = document.querySelector("#eventList");
const loopState = document.querySelector("#loopState");
const gpuState = document.querySelector("#gpuState");
const queueState = document.querySelector("#queueState");
const latencyState = document.querySelector("#latencyState");

const events = [
  ["thread/goal", "active", true],
  ["gpu/queue", "llm lease held", true],
  ["mcp/resource", "brain vault ready", false],
  ["app-server", "stream connected", false],
];

const initialMessages = [
  {
    role: "agent",
    title: "Ilhae",
    text: "GPU 큐 상태를 보고 있습니다. LLM lease가 활성화되어 있고 ComfyUI 요청이 들어오면 lease 전환 이벤트를 표시합니다.",
    tools: ["gpu.queue.status", "app-server events"],
  },
  {
    role: "user",
    title: "You",
    text: "조선 야담 이미지 생성 전에 라마서버가 자동으로 내려가는지 확인해줘.",
  },
  {
    role: "agent",
    title: "Ilhae",
    text: "요청이 들어오면 comfyui-gateway lease를 획득하고, 로컬 LLM runtime을 잠시 중지한 뒤 생성 완료 후 다시 시작하는 흐름으로 처리됩니다.",
    tools: ["comfyui-gateway", "llama-server"],
  },
];

function renderEvents() {
  eventList.innerHTML = events
    .map(
      ([name, detail, active]) => `
        <li class="${active ? "active" : ""}">
          <strong>${name}</strong>
          <span>${detail}</span>
        </li>
      `,
    )
    .join("");
}

function addMessage(message) {
  const node = document.createElement("article");
  node.className = `message ${message.role}`;
  const initials = message.role === "user" ? "Y" : "I";
  const tools = message.tools?.length
    ? `<div class="tool-row">${message.tools
        .map((tool) => `<span class="tool-pill">${tool}</span>`)
        .join("")}</div>`
    : "";

  node.innerHTML = `
    <div class="avatar" aria-hidden="true">${initials}</div>
    <div class="bubble">
      <div class="message-title">
        <span>${message.title}</span>
        <span>${new Date().toLocaleTimeString("ko-KR", {
          hour: "2-digit",
          minute: "2-digit",
        })}</span>
      </div>
      <p>${escapeHtml(message.text)}</p>
      ${tools}
    </div>
  `;
  conversation.append(node);
  conversation.scrollTop = conversation.scrollHeight;
}

function escapeHtml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function setRuntimeThinking() {
  loopState.textContent = "planning";
  gpuState.textContent = "llm active";
  queueState.textContent = "1 running";
  latencyState.textContent = "streaming";
  events.unshift(["goal loop", "agent turn started", true]);
  events.splice(5);
  renderEvents();
}

function setRuntimeDone() {
  loopState.textContent = "execution";
  gpuState.textContent = "llm active";
  queueState.textContent = "0 waiting";
  latencyState.textContent = `${Math.floor(35 + Math.random() * 30)} ms`;
  events.unshift(["goal loop", "turn completed", true]);
  events.splice(5);
  renderEvents();
}

function makeReply(text) {
  if (text.includes("GPU") || text.includes("gpu")) {
    return {
      text: "현재 LLM lease가 잡혀 있습니다. ComfyUI 작업이 들어오면 queue가 lease를 회수하고 comfyui-gateway에 넘긴 뒤 완료 이벤트를 app-server로 다시 올립니다.",
      tools: ["gpu.queue.acquire", "gpu.queue.release"],
    };
  }
  if (text.includes("야담") || text.includes("콘티")) {
    return {
      text: "1컷은 달빛 아래 선비의 실루엣, 2컷은 낡은 기와집의 닫힌 문, 3컷은 촛불 옆에 놓인 붉은 봉투로 잡으면 짧은 야담 톤이 선명해집니다.",
      tools: ["story.plan", "image.prompt"],
    };
  }
  if (text.includes("goal") || text.includes("loop")) {
    return {
      text: "현재 루프는 planning에서 execution으로 넘길 준비가 되어 있습니다. 비실행 루프에서는 deliverable 파일을 만들지 않고 Brain/Wiki 기록만 허용됩니다.",
      tools: ["goal.status", "loop.history"],
    };
  }
  return {
    text: "요청을 받았습니다. 필요한 도구를 고르고, GPU lease가 필요한 작업이면 큐 이벤트를 먼저 확인한 뒤 실행하겠습니다.",
    tools: ["tool.select", "runtime.check"],
  };
}

function submitMessage(text) {
  const trimmed = text.trim();
  if (!trimmed) {
    return;
  }
  addMessage({ role: "user", title: "You", text: trimmed });
  input.value = "";
  input.style.height = "auto";
  setRuntimeThinking();

  window.setTimeout(() => {
    const reply = makeReply(trimmed);
    addMessage({
      role: "agent",
      title: "Ilhae",
      text: reply.text,
      tools: reply.tools,
    });
    setRuntimeDone();
  }, 520);
}

composer.addEventListener("submit", (event) => {
  event.preventDefault();
  submitMessage(input.value);
});

input.addEventListener("input", () => {
  input.style.height = "auto";
  input.style.height = `${Math.min(input.scrollHeight, 140)}px`;
});

input.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    submitMessage(input.value);
  }
});

document.querySelectorAll("[data-prompt]").forEach((button) => {
  button.addEventListener("click", () => {
    submitMessage(button.dataset.prompt);
  });
});

renderEvents();
initialMessages.forEach(addMessage);
