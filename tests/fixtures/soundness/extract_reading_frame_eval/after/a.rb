class A
  def go(a)
    show(a)
    1
  end

  def show(a)
    t = eval('a * 2')
    puts t.to_s + a.to_s + 'z'
  end
end
