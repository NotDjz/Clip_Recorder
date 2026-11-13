# Fixed bugs and the tests that keep them fixed

Every bug this project has fixed, on master (Python) or on this branch, and the test that fails if
it comes back. `tests/source.rs::every_fixed_bug_still_has_its_tests` fails when a test named here
no longer exists, so a guard cannot be deleted quietly. A new fix adds a row in the same commit.

Tests marked *e2e* launch the real exe (`tests/app.rs`); everything else runs on any Windows
machine, CI included.

| Id | What broke | Tests |
|----|------------|-------|
| R01 | Keyframe interval not fps × segment length: artifacts at every concat seam | `the_keyframe_interval_equals_fps_times_the_segment_length` |
| R02 | Snapshots taken before ffmpeg flushed: segments missing their end | `packets_are_flushed_so_snapshots_are_current` |
| R03 | `-segment_wrap` shorter than the buffer: clips cut short; 5 s segments lost 1.9 s before the press | `the_segment_wrap_covers_every_buffer` |
| R04 | `format=yuv420p` with ddagrab + NVENC: "Invalid argument", no capture | `ddagrab_with_nvenc_has_no_pixel_format_filter_and_picks_the_output` |
| R05 | B-frames below 120 fps, or without `-delay 0`: video late against the audio | `nvenc_b_frames_start_at_120_fps_and_never_hold_output_back` |
| R06 | Quality regressed from the measured qp 20 / crf 18 | `quality_is_the_measured_one` |
| R07 | gdigrab captured the wrong monitor | `gdigrab_selects_the_monitor_by_its_rectangle` |
| R08 | Capture clock lost (`-debug_ts`, csv segment list missing): A/V offset grew with fps | `the_capture_prints_its_clock_and_lists_its_segments`, `only_capture_lines_carry_the_clock`, `the_clock_keeps_each_window_s_earliest_arrival_and_follows_a_step`, `the_stderr_reader_publishes_the_clock_and_keeps_the_last_words` |
| R09 | `-ss 0` dropped the whole first segment with B-frames; any seek misplaced the cut | `the_concat_never_seeks_and_cuts_at_the_selected_duration` |
| R10 | amix without resampling or with normalize: pumping volume, buzz | `the_mix_resamples_before_amix_with_normalize_off` |
| R11 | Mux re-encoded the video or ran past the shorter stream | `the_mux_copies_the_video_and_stops_at_the_shorter_stream` |
| R12 | A quote in a path broke the concat list | `the_concat_list_escapes_quotes_and_uses_forward_slashes` |
| R13 | The segment still being written was selected: corrupt frames read as desync | `the_segment_being_written_is_never_selected`, `the_concat_reads_only_snapshots_and_the_audio_window_is_the_videos` |
| R14 | A press right after a start reported as a lost clip, judged on ffmpeg's flickering liveness | `giving_up_says_why`, `a_press_right_after_the_capture_started_is_too_soon_not_an_error` |
| R15 | Fallback anchored on the oldest segment's close: 1 s offset after every settings change | `the_window_opens_where_the_oldest_segment_opened_not_where_it_closed` |
| R16 | Clip placed on file write times: audio early by up to 230 ms at 240 fps | `the_clip_is_anchored_on_capture_time_not_write_time` |
| R17 | A stale segment-list row anchored a buffer-long audio window on one segment | `a_stale_list_row_never_anchors_the_clip`, `a_broken_list_is_just_empty` |
| R18 | Audio window not the video's: overlap and echo | `enough_segments_to_cover_the_clip_length`, `the_concat_reads_only_snapshots_and_the_audio_window_is_the_videos` |
| R19 | Segments ordered by wrapped name instead of time | `segments_are_ordered_by_mtime_whatever_their_names` |
| R20 | The live ffmpeg rewrote a segment while the concat read it (TOCTOU) | `the_concat_reads_only_snapshots_and_the_audio_window_is_the_videos`, `a_snapshot_that_fails_is_an_error_and_leaves_no_copy_behind` |
| R21 | Every give-up path returned silently: a lost clip looked saved | `a_segment_folder_that_cannot_be_listed_is_an_error`, `a_snapshot_that_fails_is_an_error_and_leaves_no_copy_behind`, `a_failed_encode_reports_and_leaves_no_empty_clip`, `every_end_of_a_save_is_heard_and_seen` |
| R22 | Two saves in the same second shared a clip name: one overwrote or deleted the other's clip | `two_saves_in_the_same_second_get_their_own_clip`, `the_clip_is_judged_by_the_step_that_writes_it`, `a_failed_or_empty_clip_is_reported_and_removed` |
| R23 | A failed or timed-out mix lost the clip, or a partial mix was trusted | `a_failed_mix_keeps_the_game_sound_and_a_missing_mix_is_never_trusted`, `a_failed_mix_still_saves_the_clip_with_the_game_sound`, `a_save_muxes_the_right_audio_and_cleans_up_after_itself` |
| R24 | Audio placed by byte count: loopback and mic on different timelines | `a_silence_gap_stays_silent_where_it_really_was`, `contiguous_chunks_land_in_their_slots`, `stereo_frames_are_placed_as_frames` |
| R25 | Window clipping and dead streams in the audio reconstruction | `the_window_clips_what_is_outside_it`, `a_dead_stream_gives_nothing`, `audio_outside_the_window_gives_nothing` |
| R26 | A hole at every chunk boundary: crackling (38 discontinuities) | `jittered_arrivals_stay_contiguous` |
| R27 | The audio heal gave up 9 s after start, or reopened every tick, or forever | `a_stream_that_dies_late_is_reopened_at_once`, `a_device_that_stays_gone_backs_off_then_is_left_alone`, `a_stream_that_delivers_resets_the_backoff`, `no_device_means_nothing_to_heal` |
| R28 | A silent but open stream was reopened and came back dead | `an_open_stream_with_no_frame_yet_is_left_alone` |
| R29 | A capture restart loop stopped the audio heal | `the_health_tick_never_returns_before_healing_the_audio` |
| R30 | A device missing at a reopen stopped the heal for good, or a pinned one was replaced by the default for good | `a_pinned_device_unplugged_is_waited_for_not_replaced`, `auto_follows_the_default_device` |
| R31 | A format change mixed two formats in the history | `history_survives_only_an_unchanged_format` |
| R32 | Summing the whole history in the audio callback, or I/O in it | `the_history_keeps_its_last_seconds_by_age`, `the_audio_callback_does_no_io` |
| R33 | Audio restarted for monitor/fps/buffer changes: the round-6 desync | `only_a_device_change_restarts_the_audio` |
| R34 | Settings pre-selected the detected device: any Save pinned Auto | `auto_in_config_preselects_auto_and_saves_back_empty`, `an_untouched_form_reads_back_the_same_config` |
| R35 | An unplugged pinned device was switched to Auto by an unrelated Save | `an_unplugged_pinned_device_stays_pinned`, `a_pinned_device_is_preselected_and_saved_back_unchanged` |
| R36 | Form values parsed back wrong (the monitor label, the Auto sentinel) | `an_untouched_form_reads_back_the_same_config`, `picking_the_second_monitor_and_120_fps_reads_back_as_such` |
| R37 | "Headphones" captured "USB Headphones": substring matched first | `the_exact_device_name_wins_over_a_longer_one_listed_first` |
| R38 | A Python-era " [Loopback]" name found no device | `a_python_era_loopback_name_still_finds_its_device` |
| R39 | New devices invisible until a restart | `settings_lists_devices_on_every_open` |
| R40 | A second Apply button, a confusing subset of Save | `there_is_no_apply_button`, `the_save_button_saves` |
| R41 | An unregistered hotkey written to config.json: never retried | `a_hotkey_that_did_not_register_is_never_saved`, `a_taken_hotkey_is_refused_with_its_error_and_freed_on_drop` |
| R42 | Hotkey parsing and capture | `the_default_parses_to_ctrl_alt_r`, `letters_digits_and_function_keys`, `a_key_without_a_modifier_or_garbage_falls_back_to_the_default`, `a_captured_combination_parses_back_to_itself`, `a_capture_without_a_modifier_or_with_an_odd_key_is_refused`, `the_hotkey_button_captures_a_combination`, `escape_cancels_a_capture` |
| R43 | A hand-edited config.json stopped the app | `hand_edited_numbers_are_parsed_leniently`, `values_outside_the_options_fall_back_to_the_defaults`, `missing_keys_take_defaults_and_present_ones_are_kept`, `broken_or_missing_input_gives_the_defaults`, `a_saved_config_loads_back_identical` |
| R44 | The log grew without bound | `the_log_is_capped_keeps_the_newest_lines_and_cuts_at_a_line`, `a_cut_keeps_the_newest_half`, `the_log_cap_is_two_mebibytes` |
| R45 | Test runs swept the installed app's segment folders and killed its ffmpeg | `data_lives_in_appdata_unless_overridden`, `the_installed_app_shares_the_python_mutex_and_a_test_run_gets_its_own` |
| R46 | A chime clicked, was cut mid-note, or played on the UI thread | `a_valid_wav_as_long_as_its_last_note`, `it_starts_silent_and_rises_instead_of_clicking`, `the_failure_is_the_press_played_backwards`, `chimes_never_play_on_the_calling_thread` |
| R47 | The orphan sweep killed a stranger's reused pid, choked on a bad PID file, or raced the folder removal | `a_reused_pid_that_is_not_ffmpeg_is_never_killed`, `a_missing_or_garbage_pid_file_is_tolerated`, `an_orphan_ffmpeg_is_killed_and_waited_for`, `the_sweep_clears_every_capture_folder_and_nothing_else` |
| R48 | The job object was inert, joined late, or let ffmpeg escape | `the_job_kills_on_close_and_lets_nothing_escape_silently`, `children_are_in_the_job`, `a_force_killed_parent_takes_its_children_along`, `the_job_is_joined_before_the_first_ffmpeg`, `a_force_killed_app_takes_its_ffmpeg_along` (e2e) |
| R49 | A restart mixed two captures, or wiped a save's files or the PID file | `a_restart_wipes_only_the_live_captures_files`, `a_start_records_the_pid_wipes_the_old_segments_and_a_death_is_noticed` |
| R50 | A capture that failed to spawn was never restarted | `a_capture_that_failed_to_spawn_is_retried` |
| R51 | NVENC and ddagrab chosen from ffmpeg's lists: restart loop without an NVIDIA GPU; ddagrab into x264 | `the_pipeline_is_never_ddagrab_into_x264`, `without_a_working_encoder_the_capture_falls_back_to_x264_through_gdi`, `a_later_probe_upgrades_only_to_a_better_pipeline` |
| R52 | 120/240 fps through GDI | `gdigrab_never_captures_above_60_fps`, `above_60_fps_needs_ddagrab` |
| R53 | stderr on a 4 KB pipe stalled the capture at 240 fps | `stderr_gets_a_pipe_large_enough_for_debug_ts` |
| R54 | The banner stole focus, was captured into clips, or kept a replaced banner's pulse | `the_banner_is_styled_before_it_is_ever_shown`, `a_replaced_banner_stops_breathing`, `the_breathing_dot_runs_from_the_accent_down_and_back` |
| R55 | Unreadable text (WCAG AA) and a Save unreachable from the keyboard | `the_banner_text_is_readable`, `the_save_button_works_from_the_keyboard` |
| R56 | Start with Windows left pointing at an old install, or re-enabled by an upgrade | `start_with_windows_is_rewritten_when_ticked_and_switched_off_only_when_on`, `the_approved_entry_reads_back_as_written`, `startup_approved_reads_like_task_manager`, `the_msi_writes_the_run_value_in_hkcu_and_never_closes_the_app_itself` |
| R57 | An upgrade with the app running asked for a reboot | `quits_when_windows_ends_the_session` (e2e), `the_msi_writes_the_run_value_in_hkcu_and_never_closes_the_app_itself` |
| R58 | Two instances fought over the capture and the hotkey | `a_second_launch_exits_and_the_first_keeps_running` (e2e), `a_second_launch_opens_settings_in_the_first` (e2e) |
| R59 | An unverified or stale ffmpeg, or a stale app exe, shipped | `the_build_ships_only_a_verified_ffmpeg_and_a_fresh_exe` |
| R60 | Predictable temporary names, or a temp file written through | `random_names_are_128_bits_of_hex_and_never_repeat`, `create_new_never_overwrites`, `copy_new_copies_and_never_overwrites`, `a_save_never_deletes_a_file_it_did_not_create` |
| R61 | Log in local time to the second | `timestamps_are_utc_rfc3339` |
| R62 | The whole path: hotkey to a decodable MP4, anchored on the capture clock | `the_hotkey_saves_a_clip_that_decodes` (e2e) |
| R63 | The app crashed (0xC0000409) while quitting on WM_ENDSESSION: process::exit inside a dispatched message | `quits_when_windows_ends_the_session` (e2e), `the_session_handler_leaves_through_main` |
| R64 | Save needed two clicks: its focus scope swallowed the first | `one_click_on_save_saves` |
| R65 | A Python-era " [Loopback]" name showed as a second, phantom device in Settings | `a_python_era_loopback_name_is_not_listed_twice` |
| R66 | Ending the session while a dialog was open kept the app alive until the dialog closed | `the_session_handler_leaves_through_main` |
| R67 | Snapshots copied through an 8 KiB loop on the UI thread instead of the system copy | `copy_new_copies_and_never_overwrites` |
