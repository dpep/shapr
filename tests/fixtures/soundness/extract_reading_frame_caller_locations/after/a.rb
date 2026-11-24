class A
  def go(a)
    show(a)
    1
  end

  def show(a)
    puts caller_locations.first.to_s + a.to_s
    puts a.to_s + 'y'
  end
end
