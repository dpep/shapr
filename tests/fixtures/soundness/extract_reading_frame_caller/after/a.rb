class A
  def go(a)
    show(a)
    1
  end

  def show(a)
    puts caller.first.to_s + a.to_s
    puts a.to_s + 'y' + 'z'
  end
end
