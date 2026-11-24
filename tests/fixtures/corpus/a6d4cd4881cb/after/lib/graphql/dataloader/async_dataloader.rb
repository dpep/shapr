      def run_next_pending_lazies(run)
        smallest_depth = nil
        run.lazies_at_depth.each_key do |depth_key|
          smallest_depth ||= depth_key
          if depth_key < smallest_depth
            smallest_depth = depth_key
          end
        end

        if smallest_depth
          lazies = @lazies_at_depth.delete(smallest_depth)
          if !lazies.empty?
            begin
              run.new_queues
              lazies.each_with_index do |l, idx|
                append_job { l.value }
              end
              spawn_job_task(run) # Todo what was the last `true` condition?
              run.wait_for_queues
            ensure
              run.close_queues
            end
          end
        end
      end
