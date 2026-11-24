      @parallel_manager = manager || default_parallel_manager {
        warn 'Warning: `in_parallel` called but no parallel-capable adapter on Faraday stack'
        warn caller[2, 10].join("\n")
        nil
      }
