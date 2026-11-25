class Report
  def render(title)
    puts title
    puts "-" * 10
    puts "=" * 10
    puts "end"
  end

  def widget(kind)
    Object.const_get(kind)
  end
end
