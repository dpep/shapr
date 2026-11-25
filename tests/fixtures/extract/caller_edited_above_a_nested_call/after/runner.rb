class Runner
  def run(jobs)
    started = false
    while (work = jobs.shift)
      if work
        unless started
          started = true
          puts "start"
        end
        perform(work)
      end
    end
  end

  def perform(batch)
    return if batch.empty?

    size = batch.size
    puts size
    batch.each { |j| j.call }
    puts "done"
  end
end
