# typed: true
class A
  extend T::Sig

  def name(id)
    T.must(fetch(id)).name
  end
end
