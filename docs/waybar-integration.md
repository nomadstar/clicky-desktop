# Integración de Clicky en Barra de Estado (Waybar) y Servidor MCP

Esta guía detalla cómo habilitar el indicador de Clicky en tu barra de estado Waybar y cómo conectar modelos/agentes de terminal a Clicky mediante el protocolo estándar **MCP (Model Context Protocol)**.

---

## 1. Módulo Waybar (`custom/clicky`)

El módulo de Waybar proporciona un indicador minimalista en tu barra superior que te permite:
- **Ver el estado de Clicky en tiempo real** (Detenido, Reposo, Escuchando, Hablando, o Controlado por MCP).
- **Click izquierdo:** Abrir la ventana de Ajustes y Configuración (`clicky-desktop --settings`).
- **Click derecho:** Cerrar / Matar el proceso de forma inmediata (`clicky-desktop --kill`).

### Paso A: Añadir el módulo a `~/.config/waybar/config.jsonc`

Abre tu configuración de Waybar y añade `"custom/clicky"` a tus `modules-right` (o donde prefieras):

```jsonc
{
    // ... tus otros ajustes ...
    "modules-right": [
        "custom/clicky",
        "custom/devshell",
        "pulseaudio",
        "tray"
    ],

    "custom/clicky": {
        "format": "{icon}",
        "format-icons": {
            "running": "<span color='#38bdf8'>󰚩</span>",
            "stopped": "<span color='#64748b'>󰚩</span>"
        },
        "return-type": "json",
        "interval": 2,
        "exec": "clicky-desktop --status-json",
        "on-click": "clicky-desktop --settings",
        "on-click-right": "clicky-desktop --kill",
        "tooltip": true
    }
}
```

### Paso B: Estilos CSS en `~/.config/waybar/style.css` (Opcional)

Puedes personalizar los colores según el estado:

```css
#custom-clicky {
    padding: 0 8px;
    font-size: 15px;
    transition: all 0.3s ease;
}

#custom-clicky.running {
    color: #38bdf8;
    text-shadow: 0 0 6px rgba(56, 189, 248, 0.6);
}

#custom-clicky.stopped {
    color: #475569;
}
```

Luego recarga Waybar con:
```bash
killall -SIGUSR2 waybar
```

---

## 2. Modo Clicky MCP (Model Context Protocol)

Cualquier agente de terminal (como **Antigravity CLI**, **Claude Code**, **Cursor**, **Aider**, etc.) puede conectarse a Clicky para ver tu pantalla en tiempo real y guiarte visualmente y por voz.

### Herramientas MCP expuestas:

| Herramienta | Descripción | Parámetros |
| :--- | :--- | :--- |
| `clicky_status` | Devuelve el estado de Clicky y su versión | Ninguno |
| `clicky_screenshot` | Captura la pantalla en tiempo real y devuelve el JPEG en base64 | `save_path` (opcional) |
| `clicky_point` | Vuela el cursor triángulo azul suavemente hasta una coordenada en pantalla con un bocadillo de texto | `x`, `y`, `label` (opcional) |
| `clicky_speak` | Sintetiza y reproduce voz en tus altavoces usando Edge-TTS o ElevenLabs | `text` |
| `clicky_bubble` | Muestra un bocadillo flotante de diálogo en la pantalla | `text` |

### Configuración en Agentes

#### Para Antigravity CLI / Claude Code / Cursor:
Añade en tu archivo de configuración MCP (ej. `~/.gemini/antigravity-cli/mcp.json` o `.mcp.json`):

```json
{
  "mcpServers": {
    "clicky": {
      "command": "clicky-desktop",
      "args": ["--mcp"]
    }
  }
}
```

Si estás desarrollando en local o dentro del shell de Nix:
```json
{
  "mcpServers": {
    "clicky": {
      "command": "nix",
      "args": ["run", "github:nanixtus/clicky-desktop", "--", "--mcp"]
    }
  }
}
```

---

## 3. Comandos de Terminal Disponibles

Clicky incluye una interfaz CLI completa para controlar el daemon en segundo plano:

```bash
# Iniciar Clicky normalmente (overlay + bandeja)
clicky-desktop

# Consultar estado en consola
clicky-desktop --status

# Abrir ventana de ajustes de Clicky
clicky-desktop --settings

# Matar el proceso de Clicky
clicky-desktop --kill

# Apuntar el cursor azul a una posición de la pantalla
clicky-desktop --point 1280 400 "Mira este botón"

# Decir algo por los altavoces
clicky-desktop --speak "Compilación completada exitosamente"

# Ejecutar como servidor MCP para LLMs
clicky-desktop --mcp
```
