// nihostt WebSocket client (Browser / Node.js)
//
// Usage:
//   node js_client.js <path-to-wav-file>
//   (or open in browser console)

const WebSocket = require('ws');
const fs = require('fs');

const WS_URL = 'ws://127.0.0.1:9876/v1/ws';

async function main() {
    const wavPath = process.argv[2];
    if (!wavPath) {
        console.error('Usage: node js_client.js <wav-file>');
        process.exit(1);
    }

    const ws = new WebSocket(WS_URL);

    ws.on('open', () => {
        console.log('Connected to nihostt');

        // Send configuration
        ws.send(JSON.stringify({ type: 'configure', sample_rate: 16000 }));

        // Send WAV file as binary
        const data = fs.readFileSync(wavPath);
        // Skip WAV header (44 bytes), send raw PCM16
        const pcm = data.slice(44);
        ws.send(pcm);

        // Send stop
        setTimeout(() => {
            ws.send(JSON.stringify({ type: 'stop' }));
        }, 100);
    });

    ws.on('message', (data) => {
        const msg = JSON.parse(data.toString());
        if (msg.type === 'ready') {
            console.log('Server ready:', msg);
        } else if (msg.type === 'partial') {
            console.log('Partial:', msg.text);
        } else if (msg.type === 'final') {
            console.log('Final:', msg.text);
        } else if (msg.type === 'error') {
            console.error('Error:', msg.message);
        }
    });

    ws.on('close', () => {
        console.log('Disconnected');
        process.exit(0);
    });
}

main().catch(console.error);
