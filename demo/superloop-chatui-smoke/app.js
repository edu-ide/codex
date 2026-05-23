(function () {
  "use strict";

  var messagesEl = document.getElementById("messages");
  var form = document.getElementById("chat-form");
  var input = document.getElementById("message-input");

  function addMessage(text, role) {
    var wrapper = document.createElement("div");
    wrapper.className = "message " + role;

    var bubble = document.createElement("div");
    bubble.className = "message-bubble";

    var label = role === "user" ? "You" : "Assistant";
    bubble.innerHTML = "<strong>" + label + "</strong><br>" + escapeHtml(text);

    wrapper.appendChild(bubble);
    messagesEl.appendChild(wrapper);
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  function escapeHtml(str) {
    var div = document.createElement("div");
    div.appendChild(document.createTextNode(str));
    return div.innerHTML;
  }

  function handleSend(text) {
    if (!text.trim()) return;
    addMessage(text.trim(), "user");
    input.value = "";
    input.focus();

    // Echo back as assistant reply
    setTimeout(function () {
      addMessage("Echo: " + text.trim(), "assistant");
    }, 400);
  }

  form.addEventListener("submit", function (e) {
    e.preventDefault();
    handleSend(input.value);
  });

  input.addEventListener("keydown", function (e) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend(input.value);
    }
  });
})();
