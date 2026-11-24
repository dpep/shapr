class C
  def run(items)
    puts "start"
    items.each { |i| puts i * 2 }
    puts "done"
  end

  def other
    1
  end
end
