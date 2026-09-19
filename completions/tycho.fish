# fish completion for tycho
complete -c tycho -n "__fish_use_subcommand" -a "run" -d "Start Tycho voice listener daemon"
complete -c tycho -n "__fish_use_subcommand" -a "daemon" -d "Start Tycho in background daemon mode"
complete -c tycho -n "__fish_use_subcommand" -a "query" -d "Execute a direct voice or text query"
complete -c tycho -n "__fish_use_subcommand" -a "test-desktop" -d "Test compositor integration"
complete -c tycho -n "__fish_use_subcommand" -a "pull-models" -d "Pull latest ONNX models from HuggingFace"
complete -c tycho -n "__fish_seen_subcommand_from pull-models" -l force -d "Force redownload"
