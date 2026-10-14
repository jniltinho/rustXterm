## ADDED Requirements

### Requirement: Remote desktop view presentation
An RDP or VNC tab SHALL fill the work area edge to edge with no margin or frame. Space not covered by the scaled remote framebuffer SHALL be painted `#000000` in both themes. While connecting, the view SHALL show a centered spinner and "Connecting to <host>..." after 200 ms; on error or disconnect it SHALL show the reason and a Reconnect button centered in the view. The keyboard-grab state and release combination SHALL be shown in a 20 px overlay strip at the top of the view for 3 seconds after focus enters, then hidden until focus enters again.

#### Scenario: Letterbox
- **WHEN** a 1024×768 VNC desktop is shown scaled-to-fit in a wider work area
- **THEN** the bars beside the framebuffer are `#000000` in both themes and there is no frame around the view

#### Scenario: Grab hint
- **WHEN** focus enters an RDP view with keyboard grab on
- **THEN** a strip names the release combination for 3 seconds and then disappears
