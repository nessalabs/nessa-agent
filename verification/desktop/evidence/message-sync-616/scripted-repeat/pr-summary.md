Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | fail |
| gateway-window | pass | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:42113/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- deny webkit: frame.evaluate: Frame was detached
- release webkit: not run: deny failed
- message webkit: not run: deny failed
- context webkit: not run: deny failed
- console: console.error: [vite] TypeError: Importing a module script failed. (http://127.0.0.1:42113/@vite/client)
- console: console.error: [vite] Failed to reload /src/desktop/styles.css. This could be due to syntax errors or importing non-existent modules. (see errors above) (http://127.0.0.1:42113/@vite/client)
- 2026-10-07T05:34:03.297313Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent process started phase="process_start" elapsed_ms=0.281723
- 2026-10-07T05:34:03.297467Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase started phase="initialize"
- 2026-10-07T05:34:03.297516Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup request write finished phase="initialize" request_id=1 elapsed_ms=0.012161 outcome="success"
- 2026-10-07T05:34:03.348828Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup first frame received phase="initialize" request_id=1 elapsed_ms=51.310153
- 2026-10-07T05:34:03.348862Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase finished phase="initialize" elapsed_ms=51.396512 outcome="success"
- 2026-10-07T05:34:03.348931Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/new"
- 2026-10-07T05:34:03.349031Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/new" request_id=2 elapsed_ms=0.009526 outcome="success"
- 2026-10-07T05:34:03.399711Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/new" request_id=2 elapsed_ms=50.678001
- 2026-10-07T05:34:03.399737Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/new" elapsed_ms=50.806081000000006 outcome="success"
- 2026-10-07T05:34:03.399761Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/set_config_option"
- 2026-10-07T05:34:03.399790Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/set_config_option" request_id=3 elapsed_ms=0.008467 outcome="success"
- 2026-10-07T05:34:03.400362Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/set_config_option" request_id=3 elapsed_ms=0.571168
- 2026-10-07T05:34:03.400373Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/set_config_option" elapsed_ms=0.611672 outcome="success"
- 2026-10-07T05:34:03.400394Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent protocol startup finished phase="protocol_startup" elapsed_ms=102.95991000000001 outcome="success"
- 2026-10-07T05:34:03.400399Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent protocol ready session_id="03fb9db7-8304-4880-b716-42e3b827bef6"
- 2026-10-07T05:34:03.463934Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent prompt dispatch started session_id="03fb9db7-8304-4880-b716-42e3b827bef6" execution_id="381ce3fe-f9c0-40a9-b372-59ec99f08346"
- 2026-10-07T05:34:03.467105Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="03fb9db7-8304-4880-b716-42e3b827bef6" execution_id="381ce3fe-f9c0-40a9-b372-59ec99f08346" elapsed_ms=3.144028
- 2026-10-07T05:34:10.832715Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent prompt dispatch started session_id="03fb9db7-8304-4880-b716-42e3b827bef6" execution_id="app-fcc3d963c13756b923b31f64142ab7e7"
- 2026-10-07T05:34:10.834407Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="03fb9db7-8304-4880-b716-42e3b827bef6" execution_id="app-fcc3d963c13756b923b31f64142ab7e7" elapsed_ms=1.6380430000000001
- 2026-10-07T05:34:20.028832Z  INFO nessa_server::core::ending: nessa stopped exit_code=0 starts_again=false
- 2026-10-07T05:35:12.034686Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="70aeb98b-a802-43d3-b4ba-58d14b849cb4" execution_id="91bf8193-3039-4e77-b99b-e9098a78748c" elapsed_ms=1.706904
- 2026-10-07T05:35:14.188645Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup started phase="process_start"
- 2026-10-07T05:35:14.188992Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent process started phase="process_start" elapsed_ms=0.35101400000000005
- 2026-10-07T05:35:14.189162Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase started phase="initialize"
- 2026-10-07T05:35:14.189257Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup request write finished phase="initialize" request_id=1 elapsed_ms=0.01228 outcome="success"
- 2026-10-07T05:35:14.243983Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup first frame received phase="initialize" request_id=1 elapsed_ms=54.723835
- 2026-10-07T05:35:14.244032Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase finished phase="initialize" elapsed_ms=54.872592999999995 outcome="success"
- 2026-10-07T05:35:14.244119Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/new"
- 2026-10-07T05:35:14.244216Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/new" request_id=2 elapsed_ms=0.009552 outcome="success"
- 2026-10-07T05:35:14.299521Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/new" request_id=2 elapsed_ms=55.302902
- 2026-10-07T05:35:14.299555Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/new" elapsed_ms=55.437098 outcome="success"
- 2026-10-07T05:35:14.299594Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/set_config_option"
- 2026-10-07T05:35:14.299624Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/set_config_option" request_id=3 elapsed_ms=0.007197 outcome="success"
- 2026-10-07T05:35:14.300350Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/set_config_option" request_id=3 elapsed_ms=0.724087
- 2026-10-07T05:35:14.300376Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/set_config_option" elapsed_ms=0.782529 outcome="success"
- 2026-10-07T05:35:14.300416Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent protocol startup finished phase="protocol_startup" elapsed_ms=111.287395 outcome="success"
- 2026-10-07T05:35:14.300425Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent protocol ready session_id="4eeeb416-07ae-4da7-96f2-eb3645aedf88"
- 2026-10-07T05:35:14.437074Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent prompt dispatch started session_id="4eeeb416-07ae-4da7-96f2-eb3645aedf88" execution_id="2f7f0884-ddf6-4b37-aa5a-ddba7d2c9741"
- 2026-10-07T05:35:14.442516Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="4eeeb416-07ae-4da7-96f2-eb3645aedf88" execution_id="2f7f0884-ddf6-4b37-aa5a-ddba7d2c9741" elapsed_ms=5.409241000000001
- 2026-10-07T05:35:17.328919Z  INFO nessa_server::core::ending: nessa stopped exit_code=0 starts_again=false
- 2026-10-07T05:35:43.426754Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/new" request_id=2 elapsed_ms=0.011149 outcome="success"
- 2026-10-07T05:35:43.485074Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/new" request_id=2 elapsed_ms=58.315868
- 2026-10-07T05:35:43.485226Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/new" elapsed_ms=58.590565 outcome="success"
- 2026-10-07T05:35:43.485275Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/set_config_option"
- 2026-10-07T05:35:43.485317Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/set_config_option" request_id=3 elapsed_ms=0.01146 outcome="success"
- 2026-10-07T05:35:43.485923Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/set_config_option" request_id=3 elapsed_ms=0.604025
- 2026-10-07T05:35:43.486028Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/set_config_option" elapsed_ms=0.753187 outcome="success"
- 2026-10-07T05:35:43.486075Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent protocol startup finished phase="protocol_startup" elapsed_ms=109.505355 outcome="success"
- 2026-10-07T05:35:43.486210Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent protocol ready session_id="44f49928-3fbf-46d7-a5e5-bd7779ceae5e"
- 2026-10-07T05:35:43.549403Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent prompt dispatch started session_id="44f49928-3fbf-46d7-a5e5-bd7779ceae5e" execution_id="19f1927b-c727-4d9d-ba9d-02513269cc51"
- 2026-10-07T05:35:43.550505Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="44f49928-3fbf-46d7-a5e5-bd7779ceae5e" execution_id="19f1927b-c727-4d9d-ba9d-02513269cc51" elapsed_ms=1.0710680000000001
- 2026-10-07T05:35:45.112523Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup started phase="process_start"
- 2026-10-07T05:35:45.112745Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent process started phase="process_start" elapsed_ms=0.225021
- 2026-10-07T05:35:45.112844Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup phase started phase="initialize"
- 2026-10-07T05:35:45.112878Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup request write finished phase="initialize" request_id=1 elapsed_ms=0.006613 outcome="success"
- 2026-10-07T05:35:45.175516Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup first frame received phase="initialize" request_id=1 elapsed_ms=62.634789999999995
- 2026-10-07T05:35:45.175558Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup phase finished phase="initialize" elapsed_ms=62.714671 outcome="success"
- 2026-10-07T05:35:45.175630Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent protocol startup finished phase="protocol_startup" elapsed_ms=62.806792 outcome="error"
- 2026-10-07T05:35:45.201678Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=ada6972d-e139-46fc-b121-11a0a03eae35 error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-07T05:35:46.445708Z  INFO nessa_server::core::ending: nessa stopped exit_code=0 starts_again=false
