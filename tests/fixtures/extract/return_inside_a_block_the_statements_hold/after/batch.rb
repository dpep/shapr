class Batch
  def run(items)
    process_all(items)
    finish(items)
  end

  def process_all(items)
    puts "start"
    items.each do |item|
      return if item.nil?
      process(item.to_s)
    end
  end
end
