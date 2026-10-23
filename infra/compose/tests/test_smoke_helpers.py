"""Exercise the exact embedded SSE check without claiming Docker execution."""
import ast
from email.message import Message
from pathlib import Path
import unittest
from unittest.mock import MagicMock
import urllib.request


class SmokeHelperTests(unittest.TestCase):
    def test_sse_requires_authenticated_event_stream_and_actual_data(self):
        source = (Path(__file__).with_name('smoke.sh')).read_text().split("<<'PY_API'\n", 1)[1].split('\nPY_API', 1)[0]
        helper = next(node for node in ast.parse(source).body if isinstance(node, ast.FunctionDef) and node.name == 'check_sse')
        for content_type, body in [('text/event-stream', b'data: {"version":"v1"}\n'), ('application/json', b'{}'), ('text/event-stream', b'')]:
            with self.subTest(content_type=content_type, body=body):
                response = MagicMock(status=200)
                response.headers = Message()
                response.headers.set_type(content_type)
                response.readline.return_value = body
                opener = MagicMock()
                opener.open.return_value.__enter__.return_value = response
                namespace = dict(opener=opener, base='http://127.0.0.1:3000', urllib=urllib)
                exec(compile(ast.Module(body=[helper], type_ignores=[]), '<smoke SSE helper>', 'exec'), namespace)
                if content_type == 'text/event-stream' and body:
                    namespace['check_sse']({'Cookie': 'session-cookie'})
                    request = opener.open.call_args.args[0]
                    self.assertEqual(request.get_header('Cookie'), 'session-cookie')
                    self.assertEqual(request.get_header('Origin'), 'https://mediaflow.example.test')
                    self.assertEqual(request.get_header('Accept'), 'text/event-stream')
                else:
                    with self.assertRaises(SystemExit):
                        namespace['check_sse']({'Cookie': 'session-cookie'})


if __name__ == '__main__':
    unittest.main()
