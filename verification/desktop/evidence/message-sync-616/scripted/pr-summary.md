Verdict: fail

| Check | Chromium | WebKit |
| --- | --- | --- |
| mcp-apps-gateway | pass | pass |
| gateway-window | fail | pass |
| scripted-scenarios | pass | pass |

Relevant log lines:

- console: requestfailed: http://127.0.0.1:34085/browser/check net::ERR_ABORTED (aborted after a 204 response, #485) (harmless)
- 2026-10-07T05:31:24.779383Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup request write finished phase="initialize" request_id=1 elapsed_ms=0.011238 outcome="success"
- 2026-10-07T05:31:24.833316Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup first frame received phase="initialize" request_id=1 elapsed_ms=53.929003
- 2026-10-07T05:31:24.833364Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase finished phase="initialize" elapsed_ms=54.025315 outcome="success"
- 2026-10-07T05:31:24.833443Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/new"
- 2026-10-07T05:31:24.833540Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/new" request_id=2 elapsed_ms=0.01049 outcome="success"
- 2026-10-07T05:31:24.886865Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/new" request_id=2 elapsed_ms=53.321037000000004
- 2026-10-07T05:31:24.886907Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/new" elapsed_ms=53.465221 outcome="success"
- 2026-10-07T05:31:24.886940Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/set_config_option"
- 2026-10-07T05:31:24.886967Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/set_config_option" request_id=3 elapsed_ms=0.007852999999999999 outcome="success"
- 2026-10-07T05:31:24.887807Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/set_config_option" request_id=3 elapsed_ms=0.837769
- 2026-10-07T05:31:24.887837Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/set_config_option" elapsed_ms=0.8971610000000001 outcome="success"
- 2026-10-07T05:31:24.887864Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent protocol startup finished phase="protocol_startup" elapsed_ms=108.548688 outcome="success"
- 2026-10-07T05:31:24.887872Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent protocol ready session_id="5e0c442b-e2f4-4df0-9b9c-5d242856c72e"
- 2026-10-07T05:31:24.933182Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent prompt dispatch started session_id="5e0c442b-e2f4-4df0-9b9c-5d242856c72e" execution_id="1f651880-8857-428f-a0c2-124c2201b417"
- 2026-10-07T05:31:24.936650Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="5e0c442b-e2f4-4df0-9b9c-5d242856c72e" execution_id="1f651880-8857-428f-a0c2-124c2201b417" elapsed_ms=3.426809
- 2026-10-07T05:31:32.657477Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent prompt dispatch started session_id="5e0c442b-e2f4-4df0-9b9c-5d242856c72e" execution_id="app-6ea050046c09a27444d22c8522a51b19"
- 2026-10-07T05:31:32.658430Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="5e0c442b-e2f4-4df0-9b9c-5d242856c72e" execution_id="app-6ea050046c09a27444d22c8522a51b19" elapsed_ms=0.9059050000000001
- 2026-10-07T05:31:44.949738Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent prompt dispatch started session_id="5e0c442b-e2f4-4df0-9b9c-5d242856c72e" execution_id="app-4454d1ba3eb122a706d8c7ed453cf8f2"
- 2026-10-07T05:31:44.952745Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="5e0c442b-e2f4-4df0-9b9c-5d242856c72e" execution_id="app-4454d1ba3eb122a706d8c7ed453cf8f2" elapsed_ms=2.954205
- 2026-10-07T05:31:48.180476Z  INFO nessa_server::core::ending: nessa stopped exit_code=0 starts_again=false
- console chromium: requestfailed: http://127.0.0.1:34877/mcp-resources net::ERR_ABORTED
- 2026-10-07T05:32:11.386950Z  INFO agent_launch{launch_id=2 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="7b4fbc05-b03a-4dd4-8847-f681c7b82180" execution_id="9b70b31b-babe-4dd1-a4d4-a6abe92f2bf4" elapsed_ms=1.562194
- 2026-10-07T05:32:13.605452Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup started phase="process_start"
- 2026-10-07T05:32:13.605862Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent process started phase="process_start" elapsed_ms=0.411477
- 2026-10-07T05:32:13.606007Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase started phase="initialize"
- 2026-10-07T05:32:13.606053Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup request write finished phase="initialize" request_id=1 elapsed_ms=0.011284 outcome="success"
- 2026-10-07T05:32:13.667718Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup first frame received phase="initialize" request_id=1 elapsed_ms=61.659652
- 2026-10-07T05:32:13.667765Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase finished phase="initialize" elapsed_ms=61.758950999999996 outcome="success"
- 2026-10-07T05:32:13.667840Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/new"
- 2026-10-07T05:32:13.669489Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/new" request_id=2 elapsed_ms=1.560661 outcome="success"
- 2026-10-07T05:32:13.722756Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/new" request_id=2 elapsed_ms=53.269863
- 2026-10-07T05:32:13.722789Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/new" elapsed_ms=54.949434 outcome="success"
- 2026-10-07T05:32:13.722823Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/set_config_option"
- 2026-10-07T05:32:13.723016Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/set_config_option" request_id=3 elapsed_ms=0.16890699999999997 outcome="success"
- 2026-10-07T05:32:13.723890Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/set_config_option" request_id=3 elapsed_ms=0.8741340000000001
- 2026-10-07T05:32:13.723908Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/set_config_option" elapsed_ms=1.084898 outcome="success"
- 2026-10-07T05:32:13.723939Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent protocol startup finished phase="protocol_startup" elapsed_ms=117.96546099999999 outcome="success"
- 2026-10-07T05:32:13.723945Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent protocol ready session_id="3b1961e7-5d26-45f4-970a-7f1faf7aee46"
- 2026-10-07T05:32:13.820245Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent prompt dispatch started session_id="3b1961e7-5d26-45f4-970a-7f1faf7aee46" execution_id="9ea45790-dd60-46c8-8b46-732ead38faac"
- 2026-10-07T05:32:13.823957Z  INFO agent_launch{launch_id=4 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="3b1961e7-5d26-45f4-970a-7f1faf7aee46" execution_id="9ea45790-dd60-46c8-8b46-732ead38faac" elapsed_ms=3.682096
- 2026-10-07T05:32:16.800508Z  INFO nessa_server::core::ending: nessa stopped exit_code=0 starts_again=false
- 2026-10-07T05:32:42.914343Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/new" request_id=2 elapsed_ms=0.008022999999999999 outcome="success"
- 2026-10-07T05:32:42.972262Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/new" request_id=2 elapsed_ms=57.916238
- 2026-10-07T05:32:42.972298Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/new" elapsed_ms=58.048504 outcome="success"
- 2026-10-07T05:32:42.972330Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup phase started phase="session/set_config_option"
- 2026-10-07T05:32:42.972357Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup request write finished phase="session/set_config_option" request_id=3 elapsed_ms=0.007352 outcome="success"
- 2026-10-07T05:32:42.972933Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup first frame received phase="session/set_config_option" request_id=3 elapsed_ms=0.5747340000000001
- 2026-10-07T05:32:42.972958Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent startup phase finished phase="session/set_config_option" elapsed_ms=0.627737 outcome="success"
- 2026-10-07T05:32:42.972985Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent protocol startup finished phase="protocol_startup" elapsed_ms=112.23567399999999 outcome="success"
- 2026-10-07T05:32:42.972991Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent protocol ready session_id="98decfc2-c874-483b-b6de-f741b51668bd"
- 2026-10-07T05:32:43.066106Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent prompt dispatch started session_id="98decfc2-c874-483b-b6de-f741b51668bd" execution_id="e1d0a9de-0180-48d7-82cd-49491d357fea"
- 2026-10-07T05:32:43.067402Z  INFO agent_launch{launch_id=6 restored=false}: nessa_sdk::timing: agent first text response metric="agent_first_text_response_ms" session_id="98decfc2-c874-483b-b6de-f741b51668bd" execution_id="e1d0a9de-0180-48d7-82cd-49491d357fea" elapsed_ms=1.263201
- 2026-10-07T05:32:44.696925Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup started phase="process_start"
- 2026-10-07T05:32:44.700320Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent process started phase="process_start" elapsed_ms=3.3941779999999997
- 2026-10-07T05:32:44.700447Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup phase started phase="initialize"
- 2026-10-07T05:32:44.700501Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup request write finished phase="initialize" request_id=1 elapsed_ms=0.017074 outcome="success"
- 2026-10-07T05:32:44.767937Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup first frame received phase="initialize" request_id=1 elapsed_ms=67.434316
- 2026-10-07T05:32:44.767972Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent startup phase finished phase="initialize" elapsed_ms=67.52619299999999 outcome="success"
- 2026-10-07T05:32:44.768043Z  INFO agent_launch{launch_id=7 restored=true}: nessa_sdk::timing: agent protocol startup finished phase="protocol_startup" elapsed_ms=67.62152400000001 outcome="error"
- 2026-10-07T05:32:44.796498Z ERROR nessa_server::conversation::application::service: conversation agent opening failed conversation_id=a8ab2d4e-afde-47e2-9b54-e369191c8011 error=agent: Unsupported("provider does not support restoring a closed session")
- 2026-10-07T05:32:46.036986Z  INFO nessa_server::core::ending: nessa stopped exit_code=0 starts_again=false
